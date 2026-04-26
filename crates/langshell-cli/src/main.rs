#[tokio::main]
async fn main() -> std::process::ExitCode {
    langshell_cli::run().await
}
