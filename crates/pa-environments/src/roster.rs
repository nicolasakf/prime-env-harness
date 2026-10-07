//! Validate seat provisioning against the definition's role contract.
use anyhow::{bail, Context, Result};
use pa_types::environments::{
    EnvironmentManifest, InstanceRoster, OccupantKind, RoleName, Seat, SeatId,
    MIN_MAX_OUTPUT_TOKENS,
};
use std::collections::BTreeSet;

/// Build the minimum roster, assigning at most one eligible human seat.
/// # Errors
/// Rejects unsupported versions, invalid roles or impossible minimum rosters.
pub fn default_roster(manifest: &EnvironmentManifest) -> Result<InstanceRoster> {
    if manifest.protocol_version == 1 {
        return Ok(InstanceRoster::default());
    }
    validate_roles(manifest)?;
    let mut roster = InstanceRoster {
        seats: Vec::new(),
        ..InstanceRoster::default()
    };
    let mut human_assigned = false;
    for (role_index, role) in manifest.roles.iter().enumerate() {
        for index in 1..=role.min_seats {
            let occupant = if !human_assigned && role.occupants.contains(&OccupantKind::Human) {
                human_assigned = true;
                OccupantKind::Human
            } else {
                if !role.occupants.contains(&OccupantKind::Agent) {
                    bail!(
                        "role {} requires more human seats than supported",
                        role.name
                    );
                }
                OccupantKind::Agent
            };
            roster.seats.push(Seat {
                id: SeatId::try_from(if role.name.as_str().len() <= 60 {
                    format!("{}-{index}", role.name)
                } else {
                    format!("{}-{role_index}-{index}", &role.name.as_str()[..56])
                })
                .map_err(anyhow::Error::msg)?,
                role: role.name.clone(),
                occupant,
                model: None,
                thinking: None,
            });
        }
    }
    roster.direct_messages = manifest.direct_messages.unwrap_or(true);
    validate_roster(manifest, &roster)?;
    Ok(roster)
}

pub(crate) fn validate_roles(manifest: &EnvironmentManifest) -> Result<()> {
    if manifest.roles.is_empty() || manifest.roles.len() > 16 {
        bail!("protocol v2 requires 1–16 roles");
    }
    let mut names = BTreeSet::new();
    let mut minimum = 0;
    for role in &manifest.roles {
        if !names.insert(&role.name)
            || role.min_seats < 1
            || role.min_seats > role.max_seats
            || role.max_seats > 16
            || role.occupants.is_empty()
        {
            bail!(
                "roles require unique names, 1 ≤ min_seats ≤ max_seats ≤ 16 and allowed occupants"
            );
        }
        minimum += role.min_seats;
    }
    if minimum > 16 {
        bail!("minimum roster exceeds the 16-seat limit");
    }
    Ok(())
}

/// Enforce role membership, occupant restrictions, uniqueness and run bounds.
/// # Errors
/// Returns a diagnostic for any invalid or unsupported roster.
pub fn validate_roster(manifest: &EnvironmentManifest, roster: &InstanceRoster) -> Result<()> {
    if roster.seats.is_empty() || roster.seats.len() > 16 {
        bail!("instances require 1–16 seats");
    }
    let mut ids = BTreeSet::new();
    let mut humans = 0;
    for seat in &roster.seats {
        if !ids.insert(&seat.id) {
            bail!("seat IDs must be unique");
        }
        if seat.occupant == OccupantKind::Human {
            humans += 1;
            if seat.model.is_some() || seat.thinking.is_some() {
                bail!("human seats cannot select a model or thinking level");
            }
        }
    }
    if humans > 1 {
        bail!("at most one human seat is supported");
    }
    match manifest.protocol_version {
        1 => {
            if roster.seats.len() != 2
                || !roster.seats.iter().any(|seat| {
                    seat.id == SeatId::legacy_human()
                        && seat.role == RoleName::legacy_human()
                        && seat.occupant == OccupantKind::Human
                })
                || !roster.seats.iter().any(|seat| {
                    seat.id == SeatId::legacy_agent()
                        && seat.role == RoleName::legacy_agent()
                        && seat.occupant == OccupantKind::Agent
                })
            {
                bail!("protocol v1 requires the legacy human and agent seats");
            }
        }
        2 => {
            validate_roles(manifest)?;
            for seat in &roster.seats {
                let role = manifest
                    .roles
                    .iter()
                    .find(|role| role.name == seat.role)
                    .context("seat role is not declared")?;
                if !role.occupants.contains(&seat.occupant) {
                    bail!("role {} does not allow this occupant", role.name);
                }
            }
            for role in &manifest.roles {
                let count = roster
                    .seats
                    .iter()
                    .filter(|seat| seat.role == role.name)
                    .count();
                if count < role.min_seats || count > role.max_seats {
                    bail!(
                        "role {} needs {}–{} seats",
                        role.name,
                        role.min_seats,
                        role.max_seats
                    );
                }
            }
        }
        version => bail!("unsupported environment protocol {version}"),
    }
    let limits = &roster.limits;
    if limits.max_operations == 0
        || limits.max_wakes_per_seat == 0
        || limits.max_wall_clock_seconds == 0
        || limits.seat_turn_timeout_seconds == 0
    {
        bail!("run limits must be positive");
    }
    if limits
        .max_output_tokens
        .is_some_and(|tokens| tokens < MIN_MAX_OUTPUT_TOKENS)
    {
        bail!("max_output_tokens must be at least {MIN_MAX_OUTPUT_TOKENS}");
    }
    if manifest.direct_messages == Some(false) && roster.direct_messages {
        bail!("definition disables direct messages");
    }
    Ok(())
}
