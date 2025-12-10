mod server;

use clap::Parser;
use server::{run_with_args, CliArgs};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = CliArgs::parse();
    run_with_args(args).await
}
