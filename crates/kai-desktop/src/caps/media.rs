//! The Media capability (K23): the media players for `desktop_context` (WP14) and the transport
//! controls of `media_play_pause`, `media_next` and `media_previous` (WP16b).

use super::Fut;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Playback {
    Playing,
    Paused,
    Stopped,
    /// The player did not answer in time, or named a status MPRIS does not have.
    Unknown,
}

/// One media player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Player {
    /// What the provider addresses it by (MPRIS: the bus name); the tools never show it.
    pub id: String,
    /// The player's name as it gives it (`Spotify`, `Firefox`).
    pub name: String,
    pub playback: Playback,
    /// The track's title and artists, as the player says them (empty when it says none).
    pub title: String,
    pub artists: Vec<String>,
}

/// A transport control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    PlayPause,
    Next,
    Previous,
}

pub trait Media: Send + Sync {
    fn players(&self) -> Fut<'_, Vec<Player>>;

    /// Sends `control` to the player `id` (a [`Player::id`] from [`Self::players`]).
    fn control<'a>(&'a self, id: &'a str, control: Control) -> Fut<'a, ()>;
}
