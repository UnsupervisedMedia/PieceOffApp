#[tokio::main]
async fn main() {
    std::process::exit(pieceoff_server::run().await);
}
