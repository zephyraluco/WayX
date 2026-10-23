//! ICCCM/EWMH request handling on the internal X11 client connection.
use crate::{bridge::SharedBackend, window};
use thiserror::Error;
use x11rb::{
    connection::Connection,
    errors::{ConnectionError, ReplyError, ReplyOrIdError},
    protocol::{Event, xproto::*},
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

/// Failures while establishing or driving the internal window manager.
#[derive(Debug, Error)]
pub enum WmError {
    /// The server connection failed.
    #[error(transparent)]
    Connection(#[from] ConnectionError),
    /// An X11 request failed.
    #[error(transparent)]
    Reply(#[from] ReplyError),
    /// An X11 resource could not be allocated.
    #[error(transparent)]
    Resource(#[from] ReplyOrIdError),
    /// Another manager already owns the display.
    #[error("another window manager owns this X11 screen")]
    AlreadyManaged,
    /// Shared backend state cannot be accessed safely.
    #[error("window manager backend mutex is poisoned")]
    Poisoned,
}
x11rb::atom_manager! {
    Atoms: AtomsCookie {
        WM_S0, MANAGER, WM_STATE, WM_PROTOCOLS, WM_DELETE_WINDOW, WM_CHANGE_STATE,
        UTF8_STRING, _NET_WM_NAME, _NET_SUPPORTING_WM_CHECK, _NET_SUPPORTED,
        _NET_CLIENT_LIST, _NET_CLIENT_LIST_STACKING, _NET_ACTIVE_WINDOW,
        _NET_CLOSE_WINDOW, _NET_WM_STATE, _NET_WM_STATE_ABOVE, _NET_WM_STATE_HIDDEN,
        _NET_WM_STATE_MAXIMIZED_HORZ, _NET_WM_STATE_MAXIMIZED_VERT, _NET_WM_STATE_FULLSCREEN,
        _NET_NUMBER_OF_DESKTOPS, _NET_CURRENT_DESKTOP, _NET_DESKTOP_GEOMETRY, _NET_WORKAREA,
    }
}
/// Drives X11 window-management requests while C DDX owns server-side objects.
pub struct WindowManager {
    root: Window,
    atoms: Atoms,
    backend: SharedBackend,
    clients: Vec<Window>,
    stack: Vec<Window>,
    root_stack: Vec<Window>,
    stack_refresh: bool,
    focus: Window,
}
impl WindowManager {
    /// Acquire WM_S0 and SubstructureRedirect, rejecting an existing manager.
    pub fn new(
        conn: &RustConnection,
        screen: usize,
        owner: Window,
        backend: SharedBackend,
    ) -> Result<Self, WmError> {
        let root = &conn.setup().roots[screen];
        let atoms = Atoms::new(conn)?.reply()?;
        if conn.get_selection_owner(atoms.WM_S0)?.reply()?.owner != 0 {
            return Err(WmError::AlreadyManaged);
        }
        conn.change_window_attributes(
            root.root,
            &ChangeWindowAttributesAux::new().event_mask(
                EventMask::SUBSTRUCTURE_REDIRECT
                    | EventMask::SUBSTRUCTURE_NOTIFY
                    | EventMask::PROPERTY_CHANGE
                    | EventMask::FOCUS_CHANGE
                    | EventMask::STRUCTURE_NOTIFY,
            ),
        )?
        .check()?;
        conn.set_selection_owner(owner, atoms.WM_S0, x11rb::CURRENT_TIME)?
            .check()?;
        conn.change_property32(
            PropMode::REPLACE,
            root.root,
            atoms._NET_SUPPORTING_WM_CHECK,
            AtomEnum::WINDOW,
            &[owner],
        )?;
        conn.change_property32(
            PropMode::REPLACE,
            owner,
            atoms._NET_SUPPORTING_WM_CHECK,
            AtomEnum::WINDOW,
            &[owner],
        )?;
        conn.change_property8(
            PropMode::REPLACE,
            owner,
            atoms._NET_WM_NAME,
            atoms.UTF8_STRING,
            b"WayX",
        )?;
        let supported = [
            atoms._NET_SUPPORTING_WM_CHECK,
            atoms._NET_SUPPORTED,
            atoms._NET_WM_NAME,
            atoms._NET_CLIENT_LIST,
            atoms._NET_CLIENT_LIST_STACKING,
            atoms._NET_ACTIVE_WINDOW,
            atoms._NET_CLOSE_WINDOW,
            atoms._NET_WM_STATE,
            atoms._NET_WM_STATE_ABOVE,
            atoms._NET_WM_STATE_HIDDEN,
            atoms._NET_WM_STATE_MAXIMIZED_HORZ,
            atoms._NET_WM_STATE_MAXIMIZED_VERT,
            atoms._NET_WM_STATE_FULLSCREEN,
            atoms._NET_NUMBER_OF_DESKTOPS,
            atoms._NET_CURRENT_DESKTOP,
            atoms._NET_DESKTOP_GEOMETRY,
            atoms._NET_WORKAREA,
        ];
        conn.change_property32(
            PropMode::REPLACE,
            root.root,
            atoms._NET_SUPPORTED,
            AtomEnum::ATOM,
            &supported,
        )?;
        conn.change_property32(
            PropMode::REPLACE,
            root.root,
            atoms._NET_NUMBER_OF_DESKTOPS,
            AtomEnum::CARDINAL,
            &[1],
        )?;
        conn.change_property32(
            PropMode::REPLACE,
            root.root,
            atoms._NET_CURRENT_DESKTOP,
            AtomEnum::CARDINAL,
            &[0],
        )?;
        conn.change_property32(
            PropMode::REPLACE,
            root.root,
            atoms._NET_DESKTOP_GEOMETRY,
            AtomEnum::CARDINAL,
            &[
                u32::from(root.width_in_pixels),
                u32::from(root.height_in_pixels),
            ],
        )?;
        conn.change_property32(
            PropMode::REPLACE,
            root.root,
            atoms._NET_WORKAREA,
            AtomEnum::CARDINAL,
            &[
                0,
                0,
                u32::from(root.width_in_pixels),
                u32::from(root.height_in_pixels),
            ],
        )?;
        conn.send_event(
            false,
            root.root,
            EventMask::STRUCTURE_NOTIFY,
            ClientMessageEvent::new(32, root.root, atoms.MANAGER, [0, atoms.WM_S0, owner, 0, 0]),
        )?;
        conn.flush()?;
        Ok(Self {
            root: root.root,
            atoms,
            backend,
            clients: Vec::new(),
            stack: Vec::new(),
            root_stack: Vec::new(),
            stack_refresh: true,
            focus: 0,
        })
    }
    fn activate(
        &mut self,
        conn: &RustConnection,
        id: Window,
        time: Timestamp,
    ) -> Result<(), WmError> {
        conn.configure_window(id, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;
        conn.set_input_focus(InputFocus::POINTER_ROOT, id, time)?;
        let mut backend = self.backend.lock().map_err(|_| WmError::Poisoned)?;
        backend.focus = Some(id);
        backend.requested_focus = Some(id);
        Ok(())
    }
    fn close(&self, conn: &RustConnection, id: Window, time: Timestamp) -> Result<(), WmError> {
        let reply = conn
            .get_property(false, id, self.atoms.WM_PROTOCOLS, AtomEnum::ATOM, 0, 1024)?
            .reply()?;
        if reply
            .value32()
            .is_some_and(|mut values| values.any(|atom| atom == self.atoms.WM_DELETE_WINDOW))
        {
            conn.send_event(
                false,
                id,
                EventMask::NO_EVENT,
                ClientMessageEvent::new(
                    32,
                    id,
                    self.atoms.WM_PROTOCOLS,
                    [self.atoms.WM_DELETE_WINDOW, time, 0, 0, 0],
                ),
            )?;
        } else {
            conn.kill_client(id)?;
        }
        Ok(())
    }
    /// Handle management events before the Selection adapter consumes other events.
    pub fn handle(&mut self, conn: &RustConnection, event: &Event) -> Result<bool, WmError> {
        match event {
            Event::CreateNotify(e) if e.parent == self.root => {
                self.root_stack.retain(|id| *id != e.window);
                self.root_stack.push(e.window);
            }
            Event::DestroyNotify(e) if e.event == self.root => {
                self.root_stack.retain(|id| *id != e.window);
            }
            Event::ReparentNotify(e) => {
                self.root_stack.retain(|id| *id != e.window);
                if e.parent == self.root {
                    self.root_stack.push(e.window);
                }
            }
            Event::ConfigureNotify(e) if e.event == self.root && e.response_type & 0x80 == 0 => {
                // QueryTree/EWMH are bottom-to-top; above_sibling is the sibling below us.
                self.root_stack.retain(|id| *id != e.window);
                if e.above_sibling == 0 {
                    self.root_stack.insert(0, e.window);
                } else if let Some(index) =
                    self.root_stack.iter().position(|id| *id == e.above_sibling)
                {
                    self.root_stack.insert(index + 1, e.window);
                } else {
                    self.stack_refresh = true;
                }
            }
            Event::MapRequest(e) => {
                conn.map_window(e.window)?;
                conn.change_property32(
                    PropMode::REPLACE,
                    e.window,
                    self.atoms.WM_STATE,
                    self.atoms.WM_STATE,
                    &[1, 0],
                )?;
            }
            Event::ConfigureRequest(e) => {
                let mut attrs = ConfigureWindowAux::from_configure_request(e);
                if e.value_mask
                    .intersects(ConfigWindow::WIDTH | ConfigWindow::HEIGHT)
                {
                    let backend = self.backend.lock().map_err(|_| WmError::Poisoned)?;
                    if let Some(window) = backend.windows.get(&e.window) {
                        let width = if e.value_mask.contains(ConfigWindow::WIDTH) {
                            u32::from(e.width)
                        } else {
                            window.framebuffer.width
                        };
                        let height = if e.value_mask.contains(ConfigWindow::HEIGHT) {
                            u32::from(e.height)
                        } else {
                            window.framebuffer.height
                        };
                        let (width, height) = window.geometry.state.constrain(width, height);
                        attrs = attrs.width(width).height(height);
                    }
                }
                conn.configure_window(e.window, &attrs)?;
            }
            Event::FocusIn(e) if self.clients.contains(&e.event) => {
                self.backend.lock().map_err(|_| WmError::Poisoned)?.focus = Some(e.event);
            }
            Event::ClientMessage(e) if e.format == 32 => {
                let data = e.data.as_data32();
                if e.type_ == self.atoms._NET_ACTIVE_WINDOW {
                    self.activate(conn, e.window, data[1])?;
                } else if e.type_ == self.atoms._NET_CLOSE_WINDOW {
                    self.close(conn, e.window, data[0])?;
                } else if e.type_ == self.atoms._NET_WM_STATE {
                    let property = conn
                        .get_property(
                            false,
                            e.window,
                            self.atoms._NET_WM_STATE,
                            AtomEnum::ATOM,
                            0,
                            1024,
                        )?
                        .reply()?;
                    let mut states: Vec<_> = property
                        .value32()
                        .map(Iterator::collect)
                        .unwrap_or_default();
                    for atom in [data[1], data[2]] {
                        if atom == 0 {
                            continue;
                        }
                        let supported = [
                            self.atoms._NET_WM_STATE_ABOVE,
                            self.atoms._NET_WM_STATE_HIDDEN,
                            self.atoms._NET_WM_STATE_MAXIMIZED_HORZ,
                            self.atoms._NET_WM_STATE_MAXIMIZED_VERT,
                            self.atoms._NET_WM_STATE_FULLSCREEN,
                        ];
                        if !supported.contains(&atom) {
                            continue;
                        }
                        let add = match data[0] {
                            0 => false,
                            1 => true,
                            2 => !states.contains(&atom),
                            _ => continue,
                        };
                        states.retain(|&value| value != atom);
                        if add {
                            states.push(atom);
                        }
                    }
                    conn.change_property32(
                        PropMode::REPLACE,
                        e.window,
                        self.atoms._NET_WM_STATE,
                        AtomEnum::ATOM,
                        &states,
                    )?;
                } else if e.type_ == self.atoms.WM_CHANGE_STATE && data[0] == 3 {
                    let property = conn
                        .get_property(
                            false,
                            e.window,
                            self.atoms._NET_WM_STATE,
                            AtomEnum::ATOM,
                            0,
                            1024,
                        )?
                        .reply()?;
                    let mut states: Vec<_> = property
                        .value32()
                        .map(Iterator::collect)
                        .unwrap_or_default();
                    if !states.contains(&self.atoms._NET_WM_STATE_HIDDEN) {
                        states.push(self.atoms._NET_WM_STATE_HIDDEN);
                    }
                    conn.change_property32(
                        PropMode::REPLACE,
                        e.window,
                        self.atoms._NET_WM_STATE,
                        AtomEnum::ATOM,
                        &states,
                    )?;
                } else {
                    return Ok(false);
                }
            }
            Event::SelectionClear(e) if e.selection == self.atoms.WM_S0 => {
                return Err(WmError::AlreadyManaged);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    /// Publish supported client-list, stacking, and active-window properties.
    pub fn tick(&mut self, conn: &RustConnection) -> Result<(), WmError> {
        let (clients, focus) = {
            let backend = self.backend.lock().map_err(|_| WmError::Poisoned)?;
            let clients: Vec<_> = backend
                .windows
                .values()
                .filter(|window| {
                    window.geometry.state.flags & window::ROOTLESS != 0
                        && window.geometry.state.flags & window::OVERRIDE_REDIRECT == 0
                        && (window.mapped || window.geometry.state.flags & window::MINIMIZED != 0)
                })
                .map(|window| window.id)
                .collect();
            (clients, backend.focus.unwrap_or(0))
        };
        if clients != self.clients {
            for id in &clients {
                if !self.clients.contains(id) {
                    let attrs = conn.get_window_attributes(*id)?.reply()?;
                    conn.change_window_attributes(
                        *id,
                        &ChangeWindowAttributesAux::new()
                            .event_mask(attrs.your_event_mask | EventMask::FOCUS_CHANGE),
                    )?;
                }
            }
            conn.change_property32(
                PropMode::REPLACE,
                self.root,
                self.atoms._NET_CLIENT_LIST,
                AtomEnum::WINDOW,
                &clients,
            )?;
            self.clients = clients;
        }
        if self.stack_refresh {
            self.root_stack = conn.query_tree(self.root)?.reply()?.children;
            self.stack_refresh = false;
        }
        let stack: Vec<_> = self
            .root_stack
            .iter()
            .copied()
            .filter(|id| self.clients.contains(id))
            .collect();
        if stack != self.stack {
            conn.change_property32(
                PropMode::REPLACE,
                self.root,
                self.atoms._NET_CLIENT_LIST_STACKING,
                AtomEnum::WINDOW,
                &stack,
            )?;
            self.stack = stack;
        }
        if focus != self.focus {
            conn.change_property32(
                PropMode::REPLACE,
                self.root,
                self.atoms._NET_ACTIVE_WINDOW,
                AtomEnum::WINDOW,
                &[focus],
            )?;
            self.focus = focus;
        }
        Ok(())
    }
}
