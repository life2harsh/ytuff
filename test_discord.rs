use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = DiscordIpcClient::new("1234567890")?;
    client.connect()?;
    let activity = activity::Activity::new()
        .details("Test")
        .state("Testing");
    client.set_activity(activity)?;
    std::thread::sleep(std::time::Duration::from_secs(5));
    client.clear_activity()?;
    Ok(())
}
