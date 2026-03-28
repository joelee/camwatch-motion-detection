// Keep `main` tiny so the real application logic stays testable in the library crate.
fn main() {
    if let Err(error) = camwatch_motion_detection::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
