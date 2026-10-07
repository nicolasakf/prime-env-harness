//! Idempotent seat provisioning with stable transcripts and capabilities.
use super::{
    read, write, Context, EnvironmentHost, OccupantKind, Result, RunPhase, Seat, SeatId, Value,
};
use anyhow::bail;
use serde_json::json;
use std::time::Duration;

#[derive(Debug)]
pub(super) struct DeliveryUnavailable;
impl std::fmt::Display for DeliveryUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("seat transport unavailable; durable delivery will retry")
    }
}
impl std::error::Error for DeliveryUnavailable {}

impl EnvironmentHost {
    pub async fn provision(&self) -> Result<()> {
        if matches!(
            self.state.lock().await.phase,
            RunPhase::Stopped | RunPhase::Finished
        ) {
            return Ok(());
        }
        for seat in self.instance.roster().seats {
            if seat.occupant == OccupantKind::Agent {
                self.create_seat(&seat).await?;
            }
        }
        Ok(())
    }

    pub async fn create_seat(&self, seat: &Seat) -> Result<Value> {
        let path = self.instance.path().join("seats").join(seat.id.as_str());
        std::fs::create_dir_all(&path)?;
        let token_path = path.join("token.json");
        let token: String = if token_path.exists() {
            read(&token_path)?
        } else {
            let token = format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            );
            write(&token_path, &token)?;
            token
        };
        let mut config = self.defaults.clone();
        if let Some(script) = self.defaults.get("script").and_then(Value::as_str) {
            let source: Value = read(std::path::Path::new(script))?;
            if let Some(seat_script) = source["environmentSeats"].get(seat.id.as_str()) {
                let script_path = path.join("faux.json");
                write(&script_path, seat_script)?;
                config["script"] = json!(script_path);
            }
        }
        config["cwd"] = json!(path.join("participant"));
        config["sessionDir"] = json!(path);
        config["environmentSeat"] =
            json!({"instance":self.instance.instance.id,"seat":seat.id,"token":token});
        if let Some(model) = &seat.model {
            let (provider, model) = model
                .split_once('/')
                .context("seat model must be provider/model")?;
            config["provider"] = json!(provider);
            config["model"] = json!(model);
        }
        if let Some(thinking) = seat.thinking {
            config["thinking"] = json!(thinking.wire_name());
        }
        let transcript = if self.instance.definition.manifest.protocol_version == 1 {
            self.instance.path().join("conversation.jsonl")
        } else {
            path.join("conversation.jsonl")
        };
        let response = self.link.request_success(json!({"type":"create","sessionPath":transcript,"name":format!("{} / {} / {}",self.instance.definition.manifest.name,self.instance.instance.id,seat.id),"config":config,"lifecycle":"client_owned"}), Duration::from_secs(180)).await?;
        write(&path.join("session.json"), &response)?;
        self.seats
            .lock()
            .await
            .insert(seat.id.clone(), response.clone());
        Ok(response)
    }

    pub async fn send_seat(
        &self,
        seat: &SeatId,
        operation: &str,
        parameters: Value,
    ) -> Result<Value> {
        let session = self
            .seats
            .lock()
            .await
            .get(seat)
            .cloned()
            .context("seat is not provisioned")?;
        let command = json!({"type":"env","activeSessionId":session["activeSessionId"],"operation":operation,"parameters":parameters});
        let response = self
            .link
            .request(command.clone(), Duration::from_secs(180))
            .await
            .map_err(|_| DeliveryUnavailable)?;
        if response.success {
            return Ok(response.data.unwrap_or(Value::Null));
        }
        if response
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("Unknown active session"))
        {
            let entry = self
                .instance
                .roster()
                .seats
                .into_iter()
                .find(|entry| &entry.id == seat)
                .context("unknown seat")?;
            let session = self.create_seat(&entry).await?;
            let mut command = command;
            command["activeSessionId"] = session["activeSessionId"].clone();
            return self
                .link
                .request_success(command, Duration::from_secs(180))
                .await;
        }
        let error = response.error.unwrap_or_default();
        if error == "Session worker dropped the request"
            || error == "Session worker is not connected"
            || error == "Session worker timed out"
        {
            return Err(DeliveryUnavailable.into());
        }
        bail!("seat delivery failed: {error}")
    }

    pub async fn stop_seats(&self) -> Result<()> {
        let sessions = self.seats.lock().await.clone();
        for session in sessions.values() {
            let response = self.link.request(json!({"type":"complete_owned_session","activeSessionId":session["activeSessionId"]}), Duration::from_secs(60)).await?;
            if !response.success
                && !response
                    .error
                    .as_deref()
                    .is_some_and(|error| error.starts_with("Unknown active session"))
            {
                bail!("seat did not stop: {:?}", response.error);
            }
        }
        self.seats.lock().await.clear();
        let mut state = self.state.lock().await;
        state.busy.clear();
        self.save_state(&state)?;
        Ok(())
    }
}
