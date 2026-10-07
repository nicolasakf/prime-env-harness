fn main() {
    pa_types::memory_release::cap_thread_arenas();
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(pa_cli::environment_main(&args));
}
