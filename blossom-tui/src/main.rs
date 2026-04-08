#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    blossom_tui::run().await
}
