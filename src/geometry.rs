//! Resize direction and acknowledgements, independent of native event delivery order.
type Size = (u32, u32);
pub(crate) struct ResizeSync {
    server: Size,
    native_target: Option<Size>,
    initializing: bool,
    pending: Option<(u32, Size)>,
    sequence: u32,
    outgoing: Option<(u32, Size)>,
}
impl ResizeSync {
    pub fn new(server: Size, initial_target: Size, acknowledgement: u32) -> Self {
        Self {
            server,
            native_target: Some(initial_target),
            initializing: true,
            pending: None,
            sequence: acknowledgement,
            outgoing: None,
        }
    }
    pub fn server_update(&mut self, size: Size, acknowledgement: u32) {
        if let Some((sequence, _)) = self.pending {
            if sequence != acknowledgement {
                return;
            }
            self.pending = None;
            self.server = size;
            self.native_target = Some(size);
        } else if size != self.server {
            self.server = size;
            self.native_target = Some(size);
        }
    }
    pub fn native_resize(&mut self, size: Size) {
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        if self.native_target == Some(size) {
            self.native_target = None;
            self.initializing = false;
            // Initial native constraints can legitimately clamp the server size.
            if size == self.server {
                return;
            }
        } else if self.initializing {
            return;
        }
        if self.pending.is_some_and(|(_, pending)| pending == size)
            || (self.pending.is_none() && size == self.server)
        {
            return;
        }
        self.sequence = self.sequence.wrapping_add(1).max(1);
        self.native_target = None;
        self.pending = Some((self.sequence, size));
        self.outgoing = Some((self.sequence, size));
    }
    pub fn target(&self) -> Option<Size> {
        self.native_target
    }
    pub fn take_input(&mut self) -> Option<(u32, Size)> {
        self.outgoing.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ready() -> ResizeSync {
        let mut sync = ResizeSync::new((400, 287), (400, 287), 0);
        sync.native_resize((400, 287));
        sync
    }
    #[test]
    fn startup_transients_do_not_resize_x11() {
        let mut sync = ResizeSync::new((400, 287), (400, 287), 0);
        sync.native_resize((1424, 714));
        assert_eq!(sync.take_input(), None);
        sync.native_resize((400, 287));
        assert_eq!(sync.take_input(), None);
    }
    #[test]
    fn pending_user_resize_is_not_undone_by_old_framebuffer() {
        let mut sync = ready();
        sync.native_resize((800, 600));
        let (sequence, _) = sync.take_input().unwrap();
        sync.server_update((400, 287), 0);
        assert_eq!(sync.target(), None);
        sync.server_update((800, 600), sequence);
        sync.native_resize((800, 600));
        assert_eq!(sync.take_input(), None);
    }
    #[test]
    fn latest_request_waits_for_its_own_acknowledgement() {
        let mut sync = ready();
        sync.native_resize((800, 600));
        let (old, _) = sync.take_input().unwrap();
        sync.native_resize((900, 650));
        let (new, _) = sync.take_input().unwrap();
        sync.server_update((800, 600), old);
        assert_eq!(sync.target(), None);
        sync.server_update((896, 640), new);
        assert_eq!(sync.target(), Some((896, 640)));
        sync.native_resize((896, 640));
        assert_eq!(sync.take_input(), None);
    }
    #[test]
    fn remote_resize_and_unchanged_size_rejection_do_not_echo() {
        let mut sync = ready();
        sync.server_update((700, 500), 0);
        sync.native_resize((700, 500));
        assert_eq!(sync.take_input(), None);
        sync.native_resize((900, 650));
        let (sequence, _) = sync.take_input().unwrap();
        sync.server_update((700, 500), sequence);
        assert_eq!(sync.target(), Some((700, 500)));
    }
    #[test]
    fn initial_native_constraints_are_sent_once() {
        let mut sync = ResizeSync::new((400, 287), (384, 280), 0);
        sync.native_resize((384, 280));
        let (sequence, size) = sync.take_input().unwrap();
        assert_eq!(size, (384, 280));
        sync.server_update(size, sequence);
        sync.native_resize(size);
        assert_eq!(sync.take_input(), None);
    }
    #[test]
    fn recreated_native_window_does_not_reuse_the_old_acknowledgement() {
        let mut sync = ResizeSync::new((400, 287), (400, 287), 17);
        sync.native_resize((400, 287));
        sync.native_resize((800, 600));
        let (sequence, _) = sync.take_input().unwrap();
        assert_ne!(sequence, 17);
        sync.server_update((400, 287), 17);
        assert_eq!(sync.target(), None);
    }
}
