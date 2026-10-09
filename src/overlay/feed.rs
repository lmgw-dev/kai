//! The feed's saving (§9.3): a frame equal to the last one the page got is not sent again, since
//! the page's analysers hold it and the engine draws the same. Quiet listening in silence then
//! sends nothing.

/// What the running feed sent last. A new feed starts empty, so its first frame is always sent
/// (a new page, or one shown again, is told the levels once).
#[derive(Debug, Default)]
pub struct FeedDedup {
    last: Option<String>,
}

impl FeedDedup {
    /// The call to send for this frame, or `None` when the page holds it already.
    pub fn take(&mut self, js: String) -> Option<String> {
        if self.last.as_deref() == Some(js.as_str()) {
            return None;
        }
        self.last = Some(js.clone());
        Some(js)
    }

    /// The last call failed, so the page may not hold its frame: the next frame is sent whatever
    /// it is (review KAI-5 R5-9).
    pub fn failed(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_feed_starts_with_a_send_and_skips_equal_frames() {
        let mut d = FeedDedup::default();
        assert_eq!(d.take("a".into()).as_deref(), Some("a"));
        assert_eq!(d.take("a".into()), None, "equal to the last one sent");
        assert_eq!(d.take("a".into()), None);
        assert_eq!(d.take("b".into()).as_deref(), Some("b"));
        assert_eq!(d.take("a".into()).as_deref(), Some("a"), "different from b");
        let mut fresh = FeedDedup::default();
        assert_eq!(
            fresh.take("a".into()).as_deref(),
            Some("a"),
            "a new feed sends"
        );
    }

    #[test]
    fn a_failed_send_does_not_count_as_sent() {
        let mut d = FeedDedup::default();
        assert!(d.take("a".into()).is_some());
        d.failed();
        assert_eq!(
            d.take("a".into()).as_deref(),
            Some("a"),
            "sent again: the page may still hold the frame before it"
        );
        assert_eq!(d.take("a".into()), None);
    }
}
