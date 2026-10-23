//! Resize direction and acknowledgements, independent of native event delivery order.
type Size = (u32, u32);
pub(crate) struct ResizeSync {
    server: Size,
    native_target: Option<Size>,
    initializing: bool,
    pending: Option<(u32, Size)>,
    sequence: u32,
    outgoing: Option<(u32, Size)>,
    issued: Option<Size>,
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
            issued: None,
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
            self.issued = None;
            self.initializing = false;
            // Initial native constraints can legitimately clamp the server size.
            if size == self.server {
                return;
            }
        } else if self.initializing {
            return;
        }
        self.issued = None;
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
    /// Issue a target once while waiting for the native resize event.
    pub fn issue_target(&mut self) -> Option<Size> {
        let target = self.native_target?;
        if self.issued == Some(target) {
            return None;
        }
        self.issued = Some(target);
        Some(target)
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
    fn native_size_request_is_issued_once_per_target() {
        let mut sync = ready();
        sync.server_update((800, 600), 0);
        assert_eq!(sync.issue_target(), Some((800, 600)));
        assert_eq!(sync.issue_target(), None);
        sync.native_resize((800, 600));
        assert_eq!(sync.issue_target(), None);
        sync.native_resize((900, 650));
        let (sequence, _) = sync.take_input().unwrap();
        sync.server_update((800, 600), sequence);
        assert_eq!(sync.issue_target(), Some((800, 600)));
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

/// X border origin and native client origin share physical desktop coordinates.
/// Window decorations are excluded; winit supplies their current physical offset.
pub(crate) fn outer_for_client(
    client: (i32, i32),
    current_client: (i32, i32),
    current_outer: (i32, i32),
) -> (i32, i32) {
    (
        client
            .0
            .saturating_sub(current_client.0.saturating_sub(current_outer.0)),
        client
            .1
            .saturating_sub(current_client.1.saturating_sub(current_outer.1)),
    )
}

pub(crate) struct PositionSync {
    server: (i32, i32),
    target: Option<(i32, i32)>,
    pending: Option<(u32, (i32, i32))>,
    sequence: u32,
}
impl PositionSync {
    pub fn new(position: (i32, i32), acknowledgement: u32) -> Self {
        Self {
            server: position,
            target: Some(position),
            pending: None,
            sequence: acknowledgement,
        }
    }
    pub fn server_update(&mut self, position: (i32, i32), acknowledgement: u32) {
        if let Some((sequence, requested)) = self.pending {
            if acknowledgement != sequence {
                return;
            }
            self.pending = None;
            self.server = position;
            // A matching acknowledgement confirms the position we already
            // applied locally. Do not reapply it over a newer native movement
            // whose Moved event may still be queued.
            self.target = (position != requested).then_some(position);
        } else if self.server != position {
            self.server = position;
            self.target = Some(position);
        }
    }
    pub fn target(&self) -> Option<(i32, i32)> {
        self.target
    }
    pub fn take_target(&mut self) -> Option<(i32, i32)> {
        self.target.take()
    }
    pub fn native_move(&mut self, position: (i32, i32)) -> Option<u32> {
        if self.target == Some(position) {
            self.target = None;
            return None;
        }
        if self.pending.is_some_and(|(_, old)| old == position)
            || (self.pending.is_none() && self.server == position)
        {
            return None;
        }
        self.sequence = self.sequence.wrapping_add(1).max(1);
        self.pending = Some((self.sequence, position));
        self.target = None;
        Some(self.sequence)
    }
}

#[cfg(test)]
mod position_tests {
    use super::*;
    #[test]
    fn stale_snapshots_and_old_acknowledgements_cannot_reverse_drag() {
        let mut sync = PositionSync::new((20, 20), 41);
        assert_eq!(sync.native_move((20, 20)), None);
        let old = sync.native_move((200, 100)).unwrap();
        let latest = sync.native_move((-120, 300)).unwrap();
        sync.server_update((20, 20), 41);
        assert_eq!(sync.target(), None);
        sync.server_update((200, 100), old);
        assert_eq!(sync.target(), None);
        sync.server_update((-120, 300), latest);
        assert_eq!(sync.target(), None);
        assert_eq!(sync.native_move((-120, 300)), None);
        assert_eq!(sync.native_move((-120, 300)), None);
        sync.server_update((500, 400), latest);
        assert_eq!(sync.target(), Some((500, 400)));
        assert_eq!(sync.native_move((500, 400)), None);
        sync.server_update((500, 400), latest);
        assert_eq!(sync.target(), None);
    }
    #[test]
    fn decorations_and_negative_physical_coordinates_round_trip() {
        let client = (-1500, -80);
        let outer = (-1508, -111);
        assert_eq!(
            outer_for_client((-1200, -100), client, outer),
            (-1208, -131)
        );
        assert_eq!(outer_for_client(client, client, outer), outer);
        assert_eq!(outer_for_client(client, client, client), client);
    }
    #[test]
    fn remote_target_is_consumed_once_and_new_local_input_cancels_it() {
        let mut sync = PositionSync::new((20, 20), 0);
        sync.native_move((20, 20));
        sync.server_update((30, 40), 0);
        assert_eq!(sync.take_target(), Some((30, 40)));
        assert_eq!(sync.take_target(), None);
        sync.server_update((30, 40), 0);
        assert_eq!(sync.take_target(), None);
        sync.server_update((50, 60), 0);
        assert!(sync.native_move((100, 110)).is_some());
        assert_eq!(sync.take_target(), None);
    }
    #[test]
    fn rejected_move_is_acknowledged_without_echo() {
        let mut sync = PositionSync::new((20, 20), 0);
        sync.native_move((20, 20));
        let seq = sync.native_move((100, 100)).unwrap();
        sync.server_update((20, 20), seq);
        assert_eq!(sync.target(), Some((20, 20)));
        assert_eq!(sync.native_move((20, 20)), None);
    }
}
