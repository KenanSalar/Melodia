use std::cell::RefCell;
use std::io::{BufRead, BufReader};
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ahash::AHashMap;
use calloop::{LoopHandle, PostAction, RegistrationToken};
use rustix::pipe::{pipe_with, PipeFlags};
use sctk::data_device_manager::data_source::DataSourceData;
use sctk::data_device_manager::ReadPipe;
use sctk::globals::GlobalData;
use sctk::reexports::client::globals::{BindError, GlobalList};
use sctk::reexports::client::protocol::wl_data_device::{Event as WlDataDeviceEvent, WlDataDevice};
use sctk::reexports::client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use sctk::reexports::client::protocol::wl_data_offer::{Event as WlDataOfferEvent, WlDataOffer};
use sctk::reexports::client::protocol::wl_data_source::WlDataSource;
use sctk::reexports::client::protocol::wl_seat::WlSeat;
use sctk::reexports::client::{
    delegate_dispatch, event_created_child, Connection, Dispatch, Proxy, QueueHandle,
};
use tracing::warn;

use crate::dpi::{LogicalPosition, PhysicalPosition};
use crate::event::WindowEvent;
use crate::platform_impl::wayland;
use crate::platform_impl::wayland::event_loop::sink::EventSink;
use crate::platform_impl::wayland::state::WinitState;
use crate::platform_impl::wayland::window::WindowState;
// v0.30 has two `WindowId` types: the public `crate::window::WindowId` and
// the platform-internal `crate::platform_impl::linux::WindowId`. The
// internal type is what `wayland::make_wid()` returns and what the
// `EventSink::push_window_event` API accepts, so we use that directly.
// (Upstream PR #4009 was written against newer master where the two were
// already unified.)
use crate::platform_impl::linux::WindowId;

pub struct DataDeviceManager {
    manager: WlDataDeviceManager,
}

impl DataDeviceManager {
    pub fn new(
        globals: &GlobalList,
        queue_handle: &QueueHandle<WinitState>,
    ) -> Result<Self, BindError> {
        let manager = globals.bind(queue_handle, 1..=3, GlobalData)?;
        Ok(Self { manager })
    }

    pub fn init_seat_data_device(&self, seat: &WlSeat, queue_handle: &QueueHandle<WinitState>) {
        let data = DataDeviceData { seat: seat.clone() };
        self.manager.get_data_device(seat, queue_handle, data);
    }
}

impl Dispatch<WlDataDeviceManager, GlobalData, WinitState> for DataDeviceState {
    fn event(
        _state: &mut WinitState,
        _proxy: &WlDataDeviceManager,
        _event: <WlDataDeviceManager as Proxy>::Event,
        _data: &GlobalData,
        _conn: &Connection,
        _qhandle: &QueueHandle<WinitState>,
    ) {
    }
}

struct DataDeviceData {
    seat: WlSeat,
}

impl Dispatch<WlDataDevice, DataDeviceData, WinitState> for DataDeviceState {
    event_created_child!(WinitState, WlDataDevice, [
        0 => (WlDataOffer, Default::default())
    ]);

    fn event(
        state: &mut WinitState,
        wl_data_device: &WlDataDevice,
        event: <WlDataDevice as Proxy>::Event,
        data: &DataDeviceData,
        _conn: &Connection,
        _qhandle: &QueueHandle<WinitState>,
    ) {
        let seat_state = match state.seats.get_mut(&data.seat.id()) {
            Some(seat_state) => seat_state,
            None => {
                warn!("Received data device event {event:?} without seat");
                return;
            },
        };

        let data_device_state = match seat_state.data_device_state.as_mut() {
            Some(data_device_state) => data_device_state,
            None => {
                warn!("Received data device event {event:?} without data device state");
                return;
            },
        };

        match event {
            WlDataDeviceEvent::Enter { serial, surface, x, y, id } => {
                let Some(wl_data_offer) = id else {
                    warn!("Received data device enter event without id");
                    return;
                };

                // Every bail-out below owns destroying the offer. `finish_offer`
                // only ever sees an offer that reached `data_device_state.offer`,
                // so an early return here would drop the proxy and leak it.
                let Some(data_offer) = wl_data_offer.data::<DataOfferData>() else {
                    warn!("Received data device enter event without data offer");
                    wl_data_offer.destroy();
                    return;
                };

                let Some(mime_type) = (match data_offer.mime_types.lock() {
                    Ok(mime_types) => {
                        mime_types.iter().find(|&mime| mime == FILE_TRANSFER_MIME_TYPE).cloned()
                    },
                    Err(e) => {
                        warn!("Failed to lock MIME type vector: {e}");
                        None
                    },
                }) else {
                    warn!("Data deviced entered with no valid MIME type");
                    wl_data_offer.destroy();
                    return;
                };

                wl_data_offer.set_actions(DndAction::Copy, DndAction::Copy);
                wl_data_offer.accept(serial, Some(mime_type.clone()));

                let (read_fd, write_fd) = match pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK)
                {
                    Ok((read_fd, write_fd)) => (read_fd, write_fd),
                    Err(e) => {
                        warn!("Failed to create pipe to read data offer from: {e}");
                        wl_data_offer.destroy();
                        return;
                    },
                };

                wl_data_offer.receive(mime_type, write_fd.as_fd());
                let read_pipe = ReadPipe::from(read_fd);

                if data_device_state.read_token.is_some() {
                    warn!("Cancelling previous data device enter read");
                }
                data_device_state.release_offer();

                let window_id = wayland::make_wid(&surface);
                data_device_state.hovered_window = Some(window_id);
                data_device_state.position = physical_position(&state.windows, window_id, x, y);
                data_device_state.offer = Some(wl_data_offer);
                data_device_state.read_buffer.clear();

                let wl_data_device = wl_data_device.clone();
                data_device_state.read_token = data_device_state
                    .loop_handle
                    .insert_source(read_pipe, move |_, f, state| {
                        state.dispatched_events = true;
                        let data = wl_data_device.data::<DataDeviceData>().unwrap();

                        let seat_state = match state.seats.get_mut(&data.seat.id()) {
                            Some(seat_state) => seat_state,
                            None => return PostAction::Remove,
                        };

                        let data_device_state = match seat_state.data_device_state.as_mut() {
                            Some(data_device_state) => data_device_state,
                            None => return PostAction::Remove,
                        };

                        let f: &mut std::fs::File = unsafe { f.get_mut() };
                        let mut reader = BufReader::new(f);

                        let consumed = match reader.fill_buf() {
                            Ok(buf) => {
                                if buf.is_empty() {
                                    // Removed by the return below, so the teardown mustn't.
                                    data_device_state.read_token = None;
                                    data_device_state.read_finished(&mut state.events_sink);
                                    return PostAction::Remove;
                                } else {
                                    data_device_state.read_buffer.extend_from_slice(buf);
                                }
                                buf.len()
                            },
                            Err(e)
                                if matches!(
                                    e.kind(),
                                    std::io::ErrorKind::Interrupted
                                        | std::io::ErrorKind::WouldBlock
                                ) =>
                            {
                                return PostAction::Continue;
                            },
                            Err(_) => {
                                data_device_state.read_token = None;
                                // A drop waiting on this read has nothing coming.
                                if data_device_state.dropped {
                                    data_device_state.release_offer();
                                }
                                return PostAction::Remove;
                            },
                        };
                        reader.consume(consumed);

                        PostAction::Continue
                    })
                    .ok();
            },
            WlDataDeviceEvent::Motion { time: _, x, y } => {
                if let Some(hovered_window) = data_device_state.hovered_window {
                    let position = physical_position(&state.windows, hovered_window, x, y);
                    data_device_state.position = position;
                    if data_device_state.entered {
                        state
                            .events_sink
                            .push_window_event(WindowEvent::DragMoved { position }, hovered_window);
                    }
                }
            },
            WlDataDeviceEvent::Drop => {
                if let Some(hovered_window) = data_device_state.hovered_window {
                    if data_device_state.entered {
                        let event = WindowEvent::DragDropped {
                            paths: data_device_state.file_paths(),
                            position: data_device_state.position,
                        };
                        state.events_sink.push_window_event(event, hovered_window);
                        data_device_state.finish_offer();
                    } else if data_device_state.read_token.is_some() {
                        // The path list is still arriving, and sending what has come so far would
                        // drop the rest; the read finishes the drop once it has it all.
                        data_device_state.dropped = true;
                    } else {
                        // The read is over without entering: it named no file, or it failed.
                        data_device_state.release_offer();
                    }
                }
            },
            WlDataDeviceEvent::Leave => {
                // A drop waiting on its read is over as far as the compositor is concerned, and
                // the read still owns the offer.
                if data_device_state.dropped {
                    return;
                }
                if let Some(hovered_window) = data_device_state.hovered_window {
                    if data_device_state.entered {
                        let event =
                            WindowEvent::DragLeft { position: Some(data_device_state.position) };
                        state.events_sink.push_window_event(event, hovered_window);
                    }
                    data_device_state.release_offer();
                }
            },
            WlDataDeviceEvent::DataOffer { id: _ } => {
                // Nothing to do: `event_created_child!` above has already built
                // the proxy, and the follow-up event hands it to whichever arm
                // owns it — `Enter` for a drag, `Selection` for the clipboard.
                // Both destroy it.
            },
            WlDataDeviceEvent::Selection { id } => {
                // This data device exists for drag-and-drop; winit exposes no
                // clipboard API of its own (that is copypasta's separate data
                // device). A selection offer is therefore dead on arrival here.
                //
                // It still has to be destroyed. The compositor mints a *new*
                // `wl_data_offer` on every selection change and the client owns
                // its lifetime, so dropping the proxy silently leaks one object
                // — plus its accumulated MIME-type strings — per clipboard
                // change, for the life of the process.
                if let Some(offer) = id {
                    offer.destroy();
                }
            },
            _ => unreachable!(),
        }
    }
}

#[derive(Default)]
struct DataOfferData {
    pub mime_types: Arc<Mutex<Vec<String>>>,
}

impl Dispatch<WlDataOffer, DataOfferData, WinitState> for DataDeviceState {
    fn event(
        _state: &mut WinitState,
        _wl_data_offer: &WlDataOffer,
        event: <WlDataOffer as Proxy>::Event,
        data: &DataOfferData,
        _conn: &Connection,
        _qhandle: &QueueHandle<WinitState>,
    ) {
        match event {
            WlDataOfferEvent::SourceActions { source_actions: _ } => {},
            WlDataOfferEvent::Action { dnd_action: _ } => {},
            WlDataOfferEvent::Offer { mime_type } => match data.mime_types.lock() {
                Ok(ref mut mime_types) => {
                    mime_types.push(mime_type);
                },
                Err(e) => {
                    warn!("Failed to lock data offer mime types: {e}");
                },
            },
            _ => unreachable!(),
        }
    }
}

impl Dispatch<WlDataSource, DataSourceData, WinitState> for DataDeviceState {
    fn event(
        _state: &mut WinitState,
        _proxy: &WlDataSource,
        _event: <WlDataSource as Proxy>::Event,
        _data: &DataSourceData,
        _conn: &Connection,
        _qhandle: &QueueHandle<WinitState>,
    ) {
    }
}

#[derive(Debug)]
pub struct DataDeviceState {
    pub loop_handle: LoopHandle<'static, WinitState>,
    pub hovered_window: Option<WindowId>,
    pub offer: Option<WlDataOffer>,
    pub read_token: Option<RegistrationToken>,
    pub read_buffer: Vec<u8>,
    /// The pointer as of the last `Enter` or `Motion`, in the window's physical pixels.
    pub position: PhysicalPosition<f64>,
    /// `DragEntered` has been sent, which waits for the whole path list.
    pub entered: bool,
    /// A `Drop` arrived before the path list did.
    pub dropped: bool,
}

impl DataDeviceState {
    pub fn new(loop_handle: LoopHandle<'static, WinitState>) -> Self {
        Self {
            loop_handle,
            hovered_window: None,
            offer: None,
            read_token: None,
            read_buffer: Vec::new(),
            position: PhysicalPosition::default(),
            entered: false,
            dropped: false,
        }
    }

    /// The path list is in: the drag enters, and a drop that was waiting on it lands. A list naming
    /// no file, such as a browser's link, enters nothing, as X11's parse and Windows' `CF_HDROP`
    /// already refuse one.
    fn read_finished(&mut self, events_sink: &mut EventSink) {
        let Some(hovered_window) = self.hovered_window else {
            warn!("No window specified to push DragEntered to");
            return;
        };
        let paths = self.file_paths();
        if paths.is_empty() {
            if self.dropped {
                self.release_offer();
            }
            return;
        }
        let position = self.position;
        self.entered = true;
        events_sink.push_window_event(
            WindowEvent::DragEntered { paths: paths.clone(), position },
            hovered_window,
        );
        if self.dropped {
            events_sink
                .push_window_event(WindowEvent::DragDropped { paths, position }, hovered_window);
            self.finish_offer();
        }
    }

    fn file_paths(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if let Ok(uri_list) = std::str::from_utf8(&self.read_buffer) {
            for line in uri_list.lines() {
                // RFC 2483: comments start with `#`; entries are CRLF-terminated
                // URIs. Trim any trailing `\r` left by `String::lines` (which
                // only splits on `\n`) so prefix-matching catches it.
                let line = line.trim_end_matches('\r').trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }

                if let Some(file) = line.strip_prefix("file://") {
                    // text/uri-list URIs are percent-encoded per RFC 3986 —
                    // KDE / GNOME / Sway file managers all encode spaces as
                    // `%20`, unicode codepoints as their UTF-8 byte sequences
                    // (`%C3%A9` for `é`), etc. `PathBuf::from(file)` would
                    // store the literal `%XX` characters, so subsequent
                    // `File::open` would fail with NotFound. Decode here
                    // before handing the path off in `WindowEvent::DragDropped`.
                    paths.push(PathBuf::from(percent_decode_path(file)));
                } else {
                    warn!("Non-comment line in URI list missing prefix: '{line}'");
                }
            }
        } else {
            warn!("Failed to parse URI list from hovered file");
        }
        paths
    }

    /// Tear down after a *completed* drop: tell the source the drag finished,
    /// then release.
    ///
    /// `wl_data_offer.finish` is only valid once `wl_data_device.drop` has been
    /// received — sending it on any other path is an `invalid_finish` protocol
    /// error. It also only exists from version 3, and [`DataDeviceManager::new`]
    /// binds `1..=3`, so an older compositor really can hand back an offer that
    /// has no such request. Either mistake is grounds for the compositor to drop
    /// the connection, so both are guarded here. Cancellation goes through
    /// [`Self::release_offer`] instead.
    fn finish_offer(&mut self) {
        if let Some(offer) = self.offer.as_ref() {
            if offer.version() >= 3 {
                offer.finish();
            }
        }
        self.release_offer();
    }

    /// Release the current offer without claiming the drag completed — the
    /// cancel path, and the only correct teardown when no drop was received.
    fn release_offer(&mut self) {
        self.hovered_window = None;
        self.entered = false;
        self.dropped = false;
        if let Some(offer) = self.offer.take() {
            offer.destroy();
        }
        if let Some(token) = self.read_token.take() {
            self.loop_handle.remove(token);
        }
    }
}

/// Scales a surface-local logical point as the pointer does. A surface that isn't a window's own,
/// such as a decoration, has no scale to read and takes 1, as upstream does.
fn physical_position(
    windows: &RefCell<AHashMap<WindowId, Arc<Mutex<WindowState>>>>,
    window_id: WindowId,
    x: f64,
    y: f64,
) -> PhysicalPosition<f64> {
    let scale_factor = windows
        .borrow()
        .get(&window_id)
        .map(|window| window.lock().unwrap().scale_factor())
        .unwrap_or(1.);
    LogicalPosition::new(x, y).to_physical(scale_factor)
}

const FILE_TRANSFER_MIME_TYPE: &str = "text/uri-list";

/// Percent-decode an RFC 3986 path component. Unknown `%XX` escapes pass
/// through unchanged (vs erroring) because the consumer is filesystem
/// `open()` which will fail loudly if the path is wrong anyway, and we
/// don't want a malformed escape from the source to lose the *entire*
/// drop. Output bytes are then interpreted as UTF-8 — Linux paths are
/// byte-oriented at the kernel level, but the `text/uri-list` MIME type
/// guarantees UTF-8 (RFC 3987 IRIs notwithstanding, KDE/GNOME/Sway all
/// emit ASCII-encoded UTF-8 bytes).
fn percent_decode_path(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            match (hi, lo) {
                (Some(h), Some(l)) => {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                },
                _ => {},
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    // `from_utf8_lossy` because a corrupt source could legally produce
    // invalid UTF-8; we'd rather pass replacement characters to
    // `File::open` (which will fail with NotFound) than drop the path.
    String::from_utf8_lossy(&out).into_owned()
}

delegate_dispatch!(WinitState: [WlDataDeviceManager: GlobalData] => DataDeviceState);
delegate_dispatch!(WinitState: [WlDataDevice: DataDeviceData] => DataDeviceState);
delegate_dispatch!(WinitState: [WlDataOffer: DataOfferData] => DataDeviceState);
delegate_dispatch!(WinitState: [WlDataSource: DataSourceData] => DataDeviceState);
