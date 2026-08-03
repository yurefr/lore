use std::process;

fn main() {
    if let Err(error) = lore::interfaces::cli::run() {
        eprintln!("lore: {error}");
        process::exit(1);
    }
}
