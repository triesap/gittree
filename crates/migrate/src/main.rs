use std::process::exit;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    match gittree_migrate::run().await {
        Ok(version) => {
            println!("migrations complete: version {version}");
        }
        Err(err) => {
            eprintln!("migration failed: {err}");
            exit(1);
        }
    }
}
