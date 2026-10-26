#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    if let Err(error) = minicode::cli::main().await {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}
