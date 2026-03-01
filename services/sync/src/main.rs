use gittree_sync::{SyncConfig, SyncError, serve};
use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::process::ExitCode;

type MainRunFuture = Pin<Box<dyn Future<Output = Result<(), SyncError>>>>;
type LoadConfigFn = fn() -> Result<SyncConfig, SyncError>;
type ServeFn = fn(SyncConfig) -> MainRunFuture;

#[cfg(not(test))]
fn main() -> ExitCode {
    let mut stderr = std::io::stderr();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let exit_code = runtime.block_on(main_impl(&mut stderr));
    exit_status(exit_code)
}

async fn main_impl(stderr: &mut dyn Write) -> i32 {
    let mut load_dotenv = || {
        dotenvy::dotenv().ok();
    };
    let mut run_fn = || -> MainRunFuture { Box::pin(run()) };
    main_impl_with(&mut load_dotenv, &mut run_fn, stderr).await
}

async fn main_impl_with(
    load_dotenv: &mut dyn FnMut(),
    run_fn: &mut dyn FnMut() -> MainRunFuture,
    stderr: &mut dyn Write,
) -> i32 {
    load_dotenv();
    handle_main_result(run_fn().await, stderr)
}

async fn run() -> Result<(), SyncError> {
    run_with(load_sync_config, serve_boxed).await
}

fn load_sync_config() -> Result<SyncConfig, SyncError> {
    SyncConfig::from_env().map_err(SyncError::Config)
}

fn serve_boxed(config: SyncConfig) -> MainRunFuture {
    Box::pin(serve(config))
}

async fn run_with(load_config: LoadConfigFn, serve_fn: ServeFn) -> Result<(), SyncError> {
    let config = load_config()?;
    serve_fn(config).await
}

fn handle_main_result(result: Result<(), SyncError>, stderr: &mut dyn Write) -> i32 {
    match result {
        Ok(()) => 0,
        Err(err) => {
            let _ = writeln!(stderr, "sync service failed: {err}");
            1
        }
    }
}

fn exit_status(exit_code: i32) -> ExitCode {
    if exit_code == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(exit_code.clamp(1, u8::MAX as i32) as u8)
    }
}

#[cfg(test)]
mod main_tests;
