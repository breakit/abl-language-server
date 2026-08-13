use webspeed_language_server as wls;

#[tokio::main]
async fn main() {
    env_logger::init();
    wls::run().await;
}