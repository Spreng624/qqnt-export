fn main() {
    if let Err(error) = qqnt_export_core::run() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}
