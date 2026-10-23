//! Native IME to XIM adapter. Text travels as XIM strings, never fabricated keys.
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    sync::Arc,
};
use thiserror::Error;
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        xproto::{ChangeWindowAttributesAux, ConnectionExt, EventMask, KeyPressEvent},
    },
    rust_connection::RustConnection,
};
use xim::{
    InputContext, InputStyle, Server, ServerError, ServerHandler, UserInputContext, XimConnections,
    x11rb::{HasConnection, X11rbServer},
};

type Key = (u32, u16, u16);
fn key(ic: &InputContext) -> Key {
    (
        ic.client_win(),
        ic.input_method_id().get(),
        ic.input_context_id().get(),
    )
}
struct Context {
    // The transport owns its IC. This independent outgoing context retains
    // preedit state across native events, using the same protocol identifiers.
    outgoing: InputContext,
    window: u32,
    spot: (i16, i16),
    callbacks: bool,
}
#[derive(Default)]
struct Handler {
    contexts: HashMap<Key, Context>,
    focused: Option<Key>,
}
impl Handler {
    fn update(&mut self, ic: &InputContext) {
        let spot = ic.preedit_spot();
        let context = self.contexts.entry(key(ic)).or_insert_with(|| Context {
            outgoing: InputContext::new(
                ic.client_win(),
                ic.input_method_id(),
                ic.input_context_id(),
                ic.locale().to_owned(),
            ),
            window: 0,
            spot: (0, 0),
            callbacks: false,
        });
        context.window = ic.app_focus_win().or(ic.app_win()).map_or(0, |id| id.get());
        context.spot = (spot.x, spot.y);
        context.callbacks = ic.input_style().contains(InputStyle::PREEDIT_CALLBACKS);
    }
}
impl<S: Server<XEvent = KeyPressEvent>> ServerHandler<S> for Handler {
    type InputContextData = ();
    type InputStyleArray = [InputStyle; 4];
    fn new_ic_data(&mut self, _: &mut S, _: InputStyle) -> Result<(), ServerError> {
        Ok(())
    }
    fn input_styles(&self) -> Self::InputStyleArray {
        [
            InputStyle::PREEDIT_CALLBACKS | InputStyle::STATUS_NOTHING,
            InputStyle::PREEDIT_NOTHING | InputStyle::STATUS_NOTHING,
            InputStyle::PREEDIT_POSITION | InputStyle::STATUS_NOTHING,
            InputStyle::PREEDIT_POSITION | InputStyle::STATUS_NONE,
        ]
    }
    fn filter_events(&self) -> u32 {
        0
    }
    fn handle_connect(&mut self, _: &mut S) -> Result<(), ServerError> {
        Ok(())
    }
    fn handle_create_ic(
        &mut self,
        server: &mut S,
        ic: &mut UserInputContext<()>,
    ) -> Result<(), ServerError> {
        self.update(&ic.ic);
        server.set_event_mask(&ic.ic, 0, 0)
    }
    fn handle_destroy_ic(
        &mut self,
        _: &mut S,
        ic: UserInputContext<()>,
    ) -> Result<(), ServerError> {
        let id = key(&ic.ic);
        self.contexts.remove(&id);
        if self.focused == Some(id) {
            self.focused = None;
        }
        Ok(())
    }
    fn handle_reset_ic(
        &mut self,
        server: &mut S,
        ic: &mut UserInputContext<()>,
    ) -> Result<String, ServerError> {
        if let Some(context) = self.contexts.get_mut(&key(&ic.ic))
            && context.callbacks
        {
            server.preedit_draw(&mut context.outgoing, "")?;
        }
        Ok(String::new())
    }
    fn handle_set_ic_values(
        &mut self,
        _: &mut S,
        ic: &mut UserInputContext<()>,
    ) -> Result<(), ServerError> {
        self.update(&ic.ic);
        Ok(())
    }
    fn handle_set_focus(
        &mut self,
        server: &mut S,
        ic: &mut UserInputContext<()>,
    ) -> Result<(), ServerError> {
        if self.focused != Some(key(&ic.ic))
            && let Some(previous) = self.focused.and_then(|id| self.contexts.get_mut(&id))
            && previous.callbacks
        {
            server.preedit_draw(&mut previous.outgoing, "")?;
        }
        self.update(&ic.ic);
        self.focused = Some(key(&ic.ic));
        Ok(())
    }
    fn handle_unset_focus(
        &mut self,
        server: &mut S,
        ic: &mut UserInputContext<()>,
    ) -> Result<(), ServerError> {
        if let Some(context) = self.contexts.get_mut(&key(&ic.ic))
            && context.callbacks
        {
            server.preedit_draw(&mut context.outgoing, "")?;
        }
        if self.focused == Some(key(&ic.ic)) {
            self.focused = None;
        }
        Ok(())
    }
    fn handle_forward_event(
        &mut self,
        _: &mut S,
        _: &mut UserInputContext<()>,
        _: &KeyPressEvent,
    ) -> Result<bool, ServerError> {
        // Native physical keys already enter the DDX. Return unexpected forwarded
        // events unchanged so normal keys and shortcuts keep their XKB semantics.
        Ok(false)
    }
}
#[derive(Debug, Error)]
pub enum ImeError {
    #[error("XIM protocol: {0}")]
    Protocol(#[from] ServerError),
    #[error("X11 connection: {0}")]
    Connection(#[from] x11rb::errors::ConnectionError),
    #[error("X11 reply: {0}")]
    Reply(#[from] x11rb::errors::ReplyError),
    #[error("An XIM server named wayx is already registered")]
    AlreadyRunning,
    #[error("Invalid X11 screen index")]
    InvalidScreen,
    #[error("No focused XIM context for this native window")]
    NoContext,
    #[error("IME text exceeds the 8 KiB protocol limit")]
    TextTooLong,
}
/// A single `@im=wayx` server per X display, on the desktop event thread.
pub struct ImeBridge {
    server: X11rbServer<Arc<RustConnection>>,
    connections: XimConnections<()>,
    handler: Handler,
    root: u32,
    transports: HashMap<u32, u32>,
    composition: Option<(u32, Key)>,
    cursor_cache: RefCell<HashMap<u32, Option<(i16, i16)>>>,
    observed: RefCell<HashSet<u32>>,
}
impl ImeBridge {
    pub fn new(conn: Arc<RustConnection>, screen: usize) -> Result<Self, ImeError> {
        let root = conn
            .setup()
            .roots
            .get(screen)
            .ok_or(ImeError::InvalidScreen)?
            .root;
        let atom = conn.intern_atom(false, b"@server=wayx")?.reply()?.atom;
        if conn.get_selection_owner(atom)?.reply()?.owner != 0 {
            return Err(ImeError::AlreadyRunning);
        }
        let server = X11rbServer::init(conn, screen, "wayx", xim::ALL_LOCALES)?;
        server.conn().flush()?;
        let bridge = Self {
            server,
            connections: XimConnections::new(),
            handler: Handler::default(),
            root,
            transports: HashMap::new(),
            composition: None,
            cursor_cache: RefCell::new(HashMap::new()),
            observed: RefCell::new(HashSet::new()),
        };
        bridge.observe_window(root)?;
        Ok(bridge)
    }
    pub fn filter_event(&mut self, event: &Event) -> Result<bool, ImeError> {
        let handled = self
            .server
            .filter_event(event, &mut self.connections, &mut self.handler)?;
        if handled
            || matches!(
                event,
                Event::FocusIn(_)
                    | Event::FocusOut(_)
                    | Event::ConfigureNotify(_)
                    | Event::ReparentNotify(_)
                    | Event::DestroyNotify(_)
                    | Event::CreateNotify(_)
                    | Event::MapNotify(_)
                    | Event::UnmapNotify(_)
            )
        {
            self.cursor_cache.borrow_mut().clear();
        }
        let mut dead = Vec::new();
        if let Event::ClientMessage(message) = event {
            for id in self.handler.contexts.keys() {
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    self.transports.entry(id.0)
                {
                    entry.insert(message.window);
                    let attrs = match self.server.conn().get_window_attributes(id.0)?.reply() {
                        Ok(attrs) => attrs,
                        Err(x11rb::errors::ReplyError::X11Error(_)) => {
                            dead.push(id.0);
                            continue;
                        }
                        Err(error) => return Err(error.into()),
                    };
                    match self
                        .server
                        .conn()
                        .change_window_attributes(
                            id.0,
                            &ChangeWindowAttributesAux::new()
                                .event_mask(attrs.your_event_mask | EventMask::STRUCTURE_NOTIFY),
                        )?
                        .check()
                    {
                        Ok(()) => {}
                        Err(x11rb::errors::ReplyError::X11Error(_)) => dead.push(id.0),
                        Err(error) => return Err(error.into()),
                    }
                }
            }
        }
        if let Event::DestroyNotify(e) = event {
            self.observed.borrow_mut().remove(&e.window);
            dead.push(e.window);
            self.handler
                .contexts
                .retain(|_, context| context.window != e.window);
        }
        for client in dead {
            if let Some(window) = self.transports.remove(&client) {
                if let Some(mut connection) = self.connections.remove_connection(window) {
                    connection.disconnect(&mut self.server, &mut self.handler)?;
                }
                self.server.conn().destroy_window(window)?;
            }
        }
        let closed: Vec<_> = self
            .transports
            .iter()
            .filter_map(|(&client, &window)| {
                self.connections
                    .get_connection(window)
                    .is_none()
                    .then_some((client, window))
            })
            .collect();
        for (client, window) in closed {
            self.transports.remove(&client);
            self.server.conn().destroy_window(window)?;
        }
        Ok(handled)
    }
    fn observe_window(&self, window: u32) -> Result<bool, ImeError> {
        if self.observed.borrow().contains(&window) {
            return Ok(true);
        }
        let conn = self.server.conn();
        let attrs = match conn.get_window_attributes(window)?.reply() {
            Ok(attrs) => attrs,
            Err(x11rb::errors::ReplyError::X11Error(_)) => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        match conn
            .change_window_attributes(
                window,
                &ChangeWindowAttributesAux::new().event_mask(
                    attrs.your_event_mask
                        | EventMask::FOCUS_CHANGE
                        | EventMask::STRUCTURE_NOTIFY
                        | EventMask::SUBSTRUCTURE_NOTIFY,
                ),
            )?
            .check()
        {
            Ok(()) => {}
            Err(x11rb::errors::ReplyError::X11Error(_)) => return Ok(false),
            Err(error) => return Err(error.into()),
        }
        self.observed.borrow_mut().insert(window);
        Ok(true)
    }
    /// Rootful windows use the backend's sentinel ID 1; rootless IDs are XIDs.
    /// Require the active IC and actual X focus to belong to this native window.
    pub fn cursor(&self, native: u32) -> Result<Option<(i16, i16)>, ImeError> {
        if let Some(cursor) = self.cursor_cache.borrow().get(&native) {
            return Ok(*cursor);
        }
        let cursor = self.query_cursor(native)?;
        self.cursor_cache.borrow_mut().insert(native, cursor);
        Ok(cursor)
    }
    fn query_cursor(&self, native: u32) -> Result<Option<(i16, i16)>, ImeError> {
        let Some(context) = self
            .handler
            .focused
            .and_then(|id| self.handler.contexts.get(&id))
        else {
            return Ok(None);
        };
        if context.window <= 1 {
            return Ok(None);
        }
        let conn = self.server.conn();
        let focus = conn.get_input_focus()?.reply()?.focus;
        if focus <= 1 {
            return Ok(None);
        }
        let target = if native == 1 { self.root } else { native };
        let mut current = focus;
        let mut found_context = false;
        for _ in 0..128 {
            if !self.observe_window(current)? {
                return Ok(None);
            }
            found_context |= current == context.window;
            if current == target {
                break;
            }
            let tree = match conn.query_tree(current)?.reply() {
                Ok(tree) => tree,
                Err(x11rb::errors::ReplyError::X11Error(_)) => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            if tree.parent == 0 {
                return Ok(None);
            }
            current = tree.parent;
        }
        if !found_context || current != target {
            return Ok(None);
        }
        match conn
            .translate_coordinates(context.window, target, context.spot.0, context.spot.1)?
            .reply()
        {
            Ok(point) => Ok(Some((point.dst_x, point.dst_y))),
            Err(x11rb::errors::ReplyError::X11Error(_)) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
    pub(crate) fn focus_token(&self) -> Option<Key> {
        self.handler.focused
    }
    /// Whether the focused application draws composition feedback itself.
    pub fn inline_preedit(&self) -> bool {
        self.handler
            .focused
            .and_then(|id| self.handler.contexts.get(&id))
            .is_some_and(|context| context.callbacks)
    }
    pub fn preedit(&mut self, native: u32, text: &str) -> Result<(), ImeError> {
        if text.len() > 8192 {
            return Err(ImeError::TextTooLong);
        }
        // Text-producing native events must revalidate actual X focus even if its
        // notification is still queued. Only passive UI synchronization uses the cache.
        if self.query_cursor(native)?.is_none() {
            return Err(ImeError::NoContext);
        }
        if !text.is_empty() {
            self.composition = self.handler.focused.map(|id| (native, id));
        }
        if let Some(context) = self
            .handler
            .focused
            .and_then(|id| self.handler.contexts.get_mut(&id))
            && context.callbacks
        {
            self.server.preedit_draw(&mut context.outgoing, text)?;
        }
        self.server.conn().flush()?;
        Ok(())
    }
    /// Winit cursor offsets count UTF-8 bytes; XIM caret positions count scalars.
    pub fn preedit_at(
        &mut self,
        native: u32,
        text: &str,
        cursor: Option<(usize, usize)>,
    ) -> Result<(), ImeError> {
        self.preedit(native, text)?;
        if !text.is_empty()
            && let Some(context) = self
                .handler
                .focused
                .and_then(|id| self.handler.contexts.get(&id))
            && context.callbacks
        {
            let mut offset = cursor.map_or(0, |(_, end)| end.min(text.len()));
            while !text.is_char_boundary(offset) {
                offset -= 1;
            }
            xim::ServerCore::send_req(
                &mut self.server,
                context.outgoing.client_win(),
                xim::Request::PreeditCaret {
                    input_method_id: context.outgoing.input_method_id().get(),
                    input_context_id: context.outgoing.input_context_id().get(),
                    position: text[..offset].chars().count() as i32,
                    direction: xim::CaretDirection::AbsolutePosition,
                    style: if cursor.is_some() {
                        xim::CaretStyle::Primary
                    } else {
                        xim::CaretStyle::Invisible
                    },
                },
            )?;
            self.server.conn().flush()?;
        }
        Ok(())
    }
    /// Cancel a native composition even when X11 focus has already moved.
    pub fn cancel(&mut self, native: u32) -> Result<(), ImeError> {
        if let Some((window, id)) = self.composition
            && window == native
        {
            if let Some(context) = self.handler.contexts.get_mut(&id)
                && context.callbacks
            {
                self.server.preedit_draw(&mut context.outgoing, "")?;
            }
            self.composition = None;
            self.server.conn().flush()?;
        }
        Ok(())
    }
    pub fn commit(&mut self, native: u32, text: &str) -> Result<(), ImeError> {
        if text.len() > 8192 {
            return Err(ImeError::TextTooLong);
        }
        if self.composition.is_some() && self.composition.map(|(_, id)| id) != self.handler.focused
        {
            self.composition = None;
            return Err(ImeError::NoContext);
        }
        // Text-producing native events must revalidate actual X focus even if its
        // notification is still queued. Only passive UI synchronization uses the cache.
        if self.query_cursor(native)?.is_none() {
            return Err(ImeError::NoContext);
        }
        if let Some(context) = self
            .handler
            .focused
            .and_then(|id| self.handler.contexts.get_mut(&id))
        {
            if context.callbacks {
                self.server.preedit_draw(&mut context.outgoing, "")?;
            }
            if !text.is_empty() {
                self.server.commit(&context.outgoing, text)?;
            }
        }
        self.composition = None;
        self.server.conn().flush()?;
        Ok(())
    }
}
