//! Entry point for the immutable Kubernetes helper image.

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    match orch_driver_kubernetes::helper_main().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
