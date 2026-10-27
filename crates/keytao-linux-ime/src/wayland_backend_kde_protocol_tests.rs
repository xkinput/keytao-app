//! Exercise the real KDE dispatch handlers over an isolated Wayland socket.
//! The fake compositor follows KWin: deactivate deletes the server resources.

use super::*;
use std::{
    io::Read,
    os::unix::net::UnixStream,
    sync::{Mutex, MutexGuard, OnceLock},
};
use wayland_client::{protocol::wl_registry::WlRegistry, EventQueue};

#[path = "../../keytao-core/tests/support/smoke_fixture.rs"]
mod smoke_fixture;

struct RegistryState;
delegate_noop!(RegistryState: ignore WlRegistry);

struct Compositor {
    server: UnixStream,
    conn: Connection,
    queue: EventQueue<App>,
    app: App,
    input_method: ZwpInputMethodV1,
    _rime: MutexGuard<'static, ()>,
}

impl Compositor {
    fn new() -> Self {
        static RIME: Mutex<()> = Mutex::new(());
        let rime = RIME
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        static ENGINE: OnceLock<CoreEngine> = OnceLock::new();
        let engine = ENGINE.get_or_init(|| {
            let dir = smoke_fixture::scratch_dir("kde-protocol");
            smoke_fixture::write(&dir).expect("write isolated Rime fixture");
            let engine = CoreEngine::with_dirs(&dir, dir.to_string_lossy().into_owned());
            engine.init().expect("initialize isolated Rime fixture");
            engine
        });
        let (client, server) = UnixStream::pair().expect("Wayland socket pair");
        let conn = Connection::from_socket(client).expect("Wayland connection");
        let registry_queue = conn.new_event_queue::<RegistryState>();
        let registry = conn.display().get_registry(&registry_queue.handle(), ());
        let queue = conn.new_event_queue::<App>();
        let input_method = registry.bind::<ZwpInputMethodV1, _, _>(1, 1, &queue.handle(), ());
        let mut app = App::new(engine.create_session().expect("Rime session"), None);
        app.renderer = None;
        app.input_method = Some(input_method.clone());
        let mut compositor = Self {
            server,
            conn,
            queue,
            app,
            input_method,
            _rime: rime,
        };
        compositor.requests();
        compositor
    }

    fn event(&mut self, object: u32, opcode: u16, args: &[u32]) {
        let size = 8 + 4 * args.len();
        let mut bytes = Vec::with_capacity(size);
        bytes.extend_from_slice(&object.to_ne_bytes());
        bytes.extend_from_slice(&(((size as u32) << 16) | u32::from(opcode)).to_ne_bytes());
        for arg in args {
            bytes.extend_from_slice(&arg.to_ne_bytes());
        }
        self.server.write_all(&bytes).expect("send Wayland event");
    }

    fn dispatch(&mut self) {
        self.conn
            .prepare_read()
            .expect("prepare read")
            .read()
            .expect("read events");
        self.queue
            .dispatch_pending(&mut self.app)
            .expect("dispatch events");
    }

    fn activate(&mut self, id: u32) {
        self.event(self.input_method.id().protocol_id(), 0, &[id]);
        self.dispatch();
    }

    fn requests(&mut self) -> Vec<(u32, u16)> {
        self.conn.flush().expect("flush requests");
        self.server.set_nonblocking(true).unwrap();
        let mut bytes = Vec::new();
        match self.server.read_to_end(&mut bytes) {
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            result => {
                result.expect("read requests");
            }
        }
        self.server.set_nonblocking(false).unwrap();
        let mut requests = Vec::new();
        let mut offset = 0;
        while offset < bytes.len() {
            let object = u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap());
            let header = u32::from_ne_bytes(bytes[offset + 4..offset + 8].try_into().unwrap());
            requests.push((object, header as u16));
            offset += (header >> 16) as usize;
        }
        requests
    }

    fn install_keymap(&mut self) {
        // A self-contained keymap avoids depending on the host's XKB files.
        let keymap = b"xkb_keymap {\n\
            xkb_keycodes { minimum = 8; maximum = 255; <AC01> = 38; <SPCE> = 65; };\n\
            xkb_types { type \"ONE_LEVEL\" { modifiers = None; level_name[Level1] = \"Any\"; }; };\n\
            xkb_compatibility {};\n\
            xkb_symbols { key <AC01> { [ a ] }; key <SPCE> { [ space ] }; };\n\
        };\0";
        let mut fd = tempfile().unwrap();
        fd.write_all(keymap).unwrap();
        let keyboard = self.app.keyboard.clone().unwrap();
        <App as Dispatch<WlKeyboard, ()>>::event(
            &mut self.app,
            &keyboard,
            wl_keyboard::Event::Keymap {
                format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
                fd: fd.into(),
                size: keymap.len() as u32,
            },
            &(),
            &self.conn,
            &self.queue.handle(),
        );
        assert!(self.app.xkb_keymap.is_some());
    }
}

#[test]
fn deactivate_retires_server_deleted_objects_without_sending_requests() {
    let mut compositor = Compositor::new();
    let context_id = 0xff00_0000;
    compositor.activate(context_id);
    let context = compositor.app.context.clone().unwrap();
    let keyboard = compositor.app.keyboard.clone().unwrap();
    compositor.requests();

    // KWin deletes both resources when it sends deactivate, before the client
    // can answer. A delayed destroy/release would hit an invalid object.
    compositor.event(compositor.input_method.id().protocol_id(), 1, &[context_id]);
    compositor.event(1, 1, &[keyboard.id().protocol_id()]); // wl_display.delete_id
    compositor.dispatch();

    assert!(
        !compositor.app.active,
        "deactivation must stop input immediately"
    );
    assert!(compositor.app.context.is_none());
    assert!(compositor.app.keyboard.is_none());
    assert!(!context.is_alive(), "retire the local context proxy too");
    assert!(!keyboard.is_alive(), "retire the local keyboard proxy too");
    assert!(
        compositor.requests().is_empty(),
        "never send requests to resources KWin deleted"
    );
}

#[test]
fn replacement_does_not_send_destructors_for_the_previous_context() {
    let mut compositor = Compositor::new();
    compositor.activate(0xff00_0000);
    let old_context = compositor.app.context.clone().unwrap();
    let old_keyboard = compositor.app.keyboard.clone().unwrap();
    compositor.requests();

    compositor.activate(0xff00_0001);
    let requests = compositor.requests();
    assert!(
        requests
            .iter()
            .all(|(id, _)| *id != old_context.id().protocol_id()
                && *id != old_keyboard.id().protocol_id()),
        "old-object requests: {requests:?}"
    );
    assert!(!old_context.is_alive());
    assert!(!old_keyboard.is_alive());
    assert!(compositor.app.active);
    assert_eq!(
        compositor.app.context.as_ref().unwrap().id().protocol_id(),
        0xff00_0001
    );
    // The replacement still grabs a keyboard, so native input can resume.
    assert_eq!(requests, vec![(0xff00_0001, 9)]);
}

#[test]
fn queued_events_from_an_old_keyboard_cannot_reach_the_new_context() {
    let mut compositor = Compositor::new();
    compositor.activate(0xff00_0000);
    let old_keyboard = compositor.app.keyboard.clone().unwrap();
    compositor.requests();

    // Both events are read before dispatching; replacing the context must
    // invalidate even keyboard events already waiting in the event queue.
    compositor.event(
        compositor.input_method.id().protocol_id(),
        0,
        &[0xff00_0001],
    );
    compositor.event(old_keyboard.id().protocol_id(), 3, &[42, 100, 30, 1]);
    compositor.event(old_keyboard.id().protocol_id(), 4, &[43, 0, 0, 0, 0]);
    compositor.dispatch();
    assert_eq!(
        compositor.app.last_key_serial, 0,
        "ignore stale key serials"
    );
    assert!(compositor.app.forwarded_keys.is_empty());
    assert_eq!(compositor.requests(), vec![(0xff00_0001, 9)]);
}

#[test]
fn repeated_focus_changes_keep_the_new_keyboard_working() {
    let mut compositor = Compositor::new();
    for index in 0..100 {
        // KWin may reuse a server-allocated context id after deactivation.
        let context_id = 0xff00_0000;
        compositor.activate(context_id);
        compositor.requests();
        let keyboard_id = compositor.app.keyboard.as_ref().unwrap().id().protocol_id();
        compositor.event(keyboard_id, 3, &[index + 1, 100, 30, 1]);
        compositor.dispatch();
        // Without a keymap, the real handler forwards this press; it must
        // always target the new context instead of losing input after a switch.
        assert_eq!(compositor.app.last_key_serial, index + 1);
        assert_eq!(compositor.requests(), vec![(context_id, 10)]);
        compositor.app.key_repeat = Some(KeyRepeat {
            key: 30,
            next: Instant::now(),
            interval: Duration::from_millis(40),
        });
        compositor.event(compositor.input_method.id().protocol_id(), 1, &[context_id]);
        compositor.event(1, 1, &[keyboard_id]);
        compositor.dispatch();
        assert!(compositor.app.key_repeat.is_none());
        assert!(compositor.app.forwarded_keys.is_empty());
        assert_eq!(compositor.app.last_key_serial, 0);
        compositor.app.handle_reload();
        assert!(
            compositor.requests().is_empty(),
            "reload after deactivate must not write through the old context"
        );
    }
}

#[test]
fn a_late_deactivate_for_the_old_context_leaves_the_replacement_active() {
    let mut compositor = Compositor::new();
    compositor.activate(0xff00_0000);
    compositor.requests();
    // Both events are queued before replacement retires the old proxy.
    compositor.event(
        compositor.input_method.id().protocol_id(),
        0,
        &[0xff00_0001],
    );
    compositor.event(
        compositor.input_method.id().protocol_id(),
        1,
        &[0xff00_0000],
    );
    compositor.dispatch();
    assert!(compositor.app.active);
    assert_eq!(
        compositor.app.context.as_ref().unwrap().id().protocol_id(),
        0xff00_0001
    );
    assert_eq!(compositor.requests(), vec![(0xff00_0001, 9)]);
}

#[test]
fn chinese_composition_and_commit_work_after_each_focus_change() {
    let mut compositor = Compositor::new();
    compositor
        .app
        .session
        .select_schema(smoke_fixture::SCHEMA_ID)
        .unwrap();
    compositor.app.session.set_ascii_mode(false);
    for _ in 0..10 {
        let context_id = 0xff00_0000;
        compositor.activate(context_id);
        compositor.install_keymap();
        compositor.requests();
        let keyboard_id = compositor.app.keyboard.as_ref().unwrap().id().protocol_id();
        for serial in 1..=2 {
            compositor.event(keyboard_id, 3, &[serial, 100, 30, 1]); // a
            compositor.dispatch();
        }
        assert!(!compositor.app.session.state().candidates.is_empty());
        compositor.requests();
        compositor.event(keyboard_id, 3, &[3, 100, 57, 1]); // Space selects 甲
        compositor.dispatch();
        assert_eq!(
            compositor
                .app
                .pending_kimpanel_state
                .as_ref()
                .unwrap()
                .committed
                .as_deref(),
            Some(smoke_fixture::WORDS[0]),
        );
        let requests = compositor.requests();
        assert!(
            requests.contains(&(context_id, 1)),
            "Chinese commit must reach the client"
        );
        assert!(
            !requests.contains(&(context_id, 10)),
            "the selection key was consumed"
        );
        compositor.event(compositor.input_method.id().protocol_id(), 1, &[context_id]);
        compositor.event(1, 1, &[keyboard_id]);
        compositor.dispatch();
        assert!(compositor.requests().is_empty());
    }
}

#[test]
fn a_closed_wayland_socket_returns_an_error_instead_of_waiting_or_spinning() {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut compositor = Compositor::new();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        drop(compositor.server);
        let result = run_event_loop(
            &compositor.conn,
            &mut compositor.queue,
            &mut compositor.app,
            &runtime,
        );
        sender.send(result).unwrap();
    });
    let result = receiver
        .recv_timeout(Duration::from_secs(3))
        .expect("closed socket must wake the backend");
    assert!(result.is_err(), "disconnect must reach the recovery path");
}
