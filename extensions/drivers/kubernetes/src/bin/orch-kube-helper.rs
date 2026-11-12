//! Entry point for the immutable Kubernetes helper image.

#[cfg(target_os = "linux")]
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

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("The Kubernetes runtime helper requires Linux.");
    std::process::ExitCode::FAILURE
}
