//! Fork identity and launch composition. Environment behavior lives below CLI.
use std::path::PathBuf;

/// Launch the independently configured environment application.
#[must_use]
pub fn environment_main(args: &[String]) -> i32 {
    match configure(args) {
        Ok(args) => super::main_with_runtime(&args, &super::PrintRuntime),
        Err(error) => {
            eprintln!("Prime Environment Harness: {error}");
            1
        }
    }
}

fn configure(args: &[String]) -> anyhow::Result<Vec<String>> {
    let mut args = args.to_vec();
    let mut root = std::env::var_os("PRIME_ENV_HARNESS_DATA_DIR").map(PathBuf::from);
    if root.is_none() && std::env::var_os(pa_daemon::worker::WORKER_ROLE_ENV).is_some() {
        root = std::env::var_os(super::config::ENV_AGENT_DIR).map(PathBuf::from);
    }
    if let Some(index) = args.iter().position(|arg| arg == "--data-dir") {
        let value = args
            .get(index + 1)
            .ok_or_else(|| anyhow::anyhow!("--data-dir requires a directory"))?;
        root = Some(super::config::expand_tilde_path(value));
        args.drain(index..=index + 1);
    }
    let home = pa_types::platform::home_dir()
        .ok_or_else(|| anyhow::anyhow!("home directory unavailable; set HOME"))?;
    let root = root.unwrap_or_else(|| home.join(".prime-env-harness"));
    std::fs::create_dir_all(&root)?;
    let root = root.canonicalize()?;
    for flag in ["--daemon-socket", "--session-dir"] {
        if let Some(index) = args.iter().position(|argument| argument == flag) {
            let value = args
                .get(index + 1)
                .ok_or_else(|| anyhow::anyhow!("{flag} requires a path"))?;
            let path = super::config::expand_tilde_path(value);
            if !path.is_absolute()
                || !path.starts_with(&root)
                || path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                anyhow::bail!("{flag} must stay inside the harness data directory");
            }
        }
    }
    if root.starts_with(home.join(".prime")) || root == home {
        anyhow::bail!(
            "data directory must be separate from ordinary Prime and your home directory"
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
    }
    std::env::set_var("PRIME_ENV_HARNESS_DATA_DIR", &root);
    std::env::set_var(super::config::ENV_AGENT_DIR, &root);
    std::env::set_var(super::config::ENV_SESSION_DIR, root.join("sessions"));
    std::env::remove_var(super::config::ENV_LEGACY_SESSION_DIR);
    std::env::set_var(super::config::ENV_DAEMON_SOCKET, root.join("daemon.sock"));
    std::env::set_var("PRIME_ENV_HARNESS_APP", "1");
    if args.first().is_some_and(|arg| arg == "update") {
        anyhow::bail!("fork updates are built from nicolasakf/prime-env-harness; upstream installers are disabled");
    }
    Ok(args)
}
