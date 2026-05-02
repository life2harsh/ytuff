use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
use std::sync::Mutex;

pub struct DiscordRpc {
    client: Option<DiscordIpcClient>,
}

impl DiscordRpc {
    pub fn new(client_id: &str) -> Option<Self> {
        match DiscordIpcClient::new(client_id) {
            Ok(mut client) => match client.connect() {
                Ok(()) => {
                    eprintln!("Discord RPC: connected");
                    Some(Self {
                        client: Some(client),
                    })
                }
                Err(e) => {
                    eprintln!("Discord RPC: connect failed: {}", e);
                    None
                }
            },
            Err(e) => {
                eprintln!("Discord RPC: client creation failed: {}", e);
                None
            }
        }
    }

    pub fn update(&mut self, title: &str, artist: Option<&str>) {
        if let Some(ref mut client) = self.client {
            let activity = activity::Activity::new()
                .details(title)
                .state(artist.unwrap_or(""));
            if let Err(e) = client.set_activity(activity) {
                eprintln!("Discord RPC: set_activity failed: {}", e);
            }
        }
    }

    pub fn clear(&mut self) {
        if let Some(ref mut client) = self.client {
            if let Err(e) = client.clear_activity() {
                eprintln!("Discord RPC: clear failed: {}", e);
            }
        }
    }
}

pub type SharedDiscordRpc = Mutex<Option<DiscordRpc>>;

pub fn init() -> SharedDiscordRpc {
    Mutex::new(DiscordRpc::new("1234567890"))
}
