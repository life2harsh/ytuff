use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const DISCORD_CLIENT_ID: &str = "1500144232965345412";
const RECONNECT_COOLDOWN: Duration = Duration::from_secs(10);

pub struct DiscordRpc {
    client: Option<DiscordIpcClient>,
    next_reconnect_at: Instant,
}

impl DiscordRpc {
    pub fn new() -> Self {
        let mut rpc = Self {
            client: None,
            next_reconnect_at: Instant::now(),
        };
        rpc.connect_now();
        rpc
    }

    fn connect_now(&mut self) -> bool {
        match DiscordIpcClient::new(DISCORD_CLIENT_ID) {
            Ok(mut client) => match client.connect() {
                Ok(()) => {
                    eprintln!("Discord RPC: connected");
                    self.client = Some(client);
                    true
                }
                Err(e) => {
                    eprintln!("Discord RPC: connect failed: {}", e);
                    self.client = None;
                    self.next_reconnect_at = Instant::now() + RECONNECT_COOLDOWN;
                    false
                }
            },
            Err(e) => {
                eprintln!("Discord RPC: client creation failed: {}", e);
                self.client = None;
                self.next_reconnect_at = Instant::now() + RECONNECT_COOLDOWN;
                false
            }
        }
    }

    fn ensure_connected(&mut self) -> bool {
        if self.client.is_some() {
            return true;
        }

        if Instant::now() >= self.next_reconnect_at {
            return self.connect_now();
        }
        false
    }
    pub fn update(&mut self, title: &str, artist: Option<&str>) {
        if !self.ensure_connected() {
            return;
        }

        let mut rpc_activity = activity::Activity::new().details(title);

        if let Some(artist) = artist.map(str::trim).filter(|value| !value.is_empty()) {
            rpc_activity = rpc_activity.state(artist);
        }

        if let Some(client) = self.client.as_mut() {
            if let Err(e) = client.set_activity(rpc_activity) {
                eprintln!("Discord RPC: set_activity failed: {}", e);
                self.client = None;
                self.next_reconnect_at = Instant::now() + RECONNECT_COOLDOWN;
            }
        }
    }

    pub fn clear(&mut self) {
        if !self.ensure_connected() {
            return;
        }

        if let Some(client) = self.client.as_mut() {
            if let Err(e) = client.clear_activity() {
                eprintln!("Discord RPC: clear failed: {}", e);
                self.client = None;
                self.next_reconnect_at = Instant::now() + RECONNECT_COOLDOWN;
            }
        }
    }
}
pub type SharedDiscordRpc = Mutex<Option<DiscordRpc>>;
pub fn init() -> SharedDiscordRpc {
    Mutex::new(Some(DiscordRpc::new()))
}
