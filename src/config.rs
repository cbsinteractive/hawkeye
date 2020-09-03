use color_eyre::{eyre::eyre, Result};
use std::path::PathBuf;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt(
    name = "video-slate-detector",
    about = "Detects slate image and triggers URL request."
)]
pub struct AppConfig {
    // Path to the slate image
    #[structopt(parse(from_os_str))]
    pub slate_path: PathBuf,

    // Port to listen for the RTP stream
    #[structopt(short = "i", long = "ingest-port", default_value = "5000")]
    pub ingest_port: u32,

    // URL to call when the slate is detected
    #[structopt(parse(try_from_str = parse_url))]
    pub url: String,

    // Method to use in the call
    #[structopt(short = "m", long = "http-method", default_value = "POST", possible_values = &["POST", "GET", "DELETE", "PUT", "PATCH"])]
    pub method: String,

    // The raw payload that should be sent to the backend API
    #[structopt(short = "p", long = "payload", default_value = "")]
    pub payload: String,
}

fn parse_url(url: &str) -> Result<String> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(String::from(url))
    } else {
        Err(eyre!("{} not recognized as a valid URL!", url))
    }
}
