fn main() {
    if let Err(error) = buzz_acp::run() {
        eprintln!("buzz-acp: {error}");
        std::process::exit(1);
    }
}
