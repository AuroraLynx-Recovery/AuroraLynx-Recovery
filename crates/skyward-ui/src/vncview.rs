//! A VNC framebuffer inside the recovery UI.
//!
//! The protocol engine is [vnc-rs](https://crates.io/crates/vnc-rs). A browser
//! that draws on another machine (WRP's `-vnc`, usually port 5901) is the
//! picture this page shows. This phone does not run the browser.

use crate::canvas::Canvas;
use crate::theme::Theme;
use crate::widgets;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{self, UnboundedSender};
use vnc::{
    ClientKeyEvent, ClientMouseEvent, PixelFormat, Rect, VncConnector, VncEncoding, VncEvent,
    X11Event,
};

const MAX_SIDE: u32 = 8192;
const MAX_PIXELS: u64 = 8_294_400;

/// Where the remote picture is drawn, in screen pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The area above the disconnect bar, and above the keyboard when it is up.
pub fn slot(t: &Theme, keyboard: bool) -> Slot {
    let m = &t.m;
    let kb = if keyboard {
        widgets::keyboard_height(t)
    } else {
        0.0
    };
    let y = m.content_top;
    let bottom = if keyboard { t.m.h as f32 } else { t.dock_top() } - kb - m.dp(72.0);
    Slot {
        x: 0.0,
        y,
        w: m.w as f32,
        h: (bottom - y).max(1.0),
    }
}

/// The picture inside `slot`, letterboxed so each remote pixel stays square.
pub fn letterbox(slot: Slot, remote_w: u32, remote_h: u32) -> Slot {
    if remote_w == 0 || remote_h == 0 || slot.w < 1.0 || slot.h < 1.0 {
        return Slot {
            w: 0.0,
            h: 0.0,
            ..slot
        };
    }
    let scale = (slot.w / remote_w as f32).min(slot.h / remote_h as f32);
    let w = (remote_w as f32 * scale).max(1.0);
    let h = (remote_h as f32 * scale).max(1.0);
    Slot {
        x: slot.x + (slot.w - w) * 0.5,
        y: slot.y + (slot.h - h) * 0.5,
        w,
        h,
    }
}

/// The remote pixel under a screen point, if it lands on the picture.
pub fn map_point(slot: Slot, remote_w: u32, remote_h: u32, x: f32, y: f32) -> Option<(u16, u16)> {
    let placed = letterbox(slot, remote_w, remote_h);
    if remote_w == 0
        || remote_h == 0
        || x < placed.x
        || y < placed.y
        || x >= placed.x + placed.w
        || y >= placed.y + placed.h
    {
        return None;
    }
    let px = ((x - placed.x) / placed.w * remote_w as f32) as u32;
    let py = ((y - placed.y) / placed.h * remote_h as f32) as u32;
    Some((px.min(remote_w - 1) as u16, py.min(remote_h - 1) as u16))
}

/// `host`, `host:port`, or nothing when the field is empty. A bare host uses 5900.
pub fn endpoint(host: &str) -> Option<String> {
    let host = host.trim();
    if host.is_empty() {
        return None;
    }
    if host.parse::<std::net::SocketAddr>().is_ok() {
        return Some(host.to_string());
    }
    match host.rsplit_once(':') {
        Some((name, port)) if !name.is_empty() && port.parse::<u16>().is_ok() => {
            Some(host.to_string())
        }
        _ => Some(format!("{host}:5900")),
    }
}

/// One remote frame, straight RGBA.
pub struct Raster {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Raster {
    fn new() -> Self {
        Self {
            w: 0,
            h: 0,
            rgba: Vec::new(),
        }
    }

    fn resize(&mut self, w: u32, h: u32) -> bool {
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || w as u64 * h as u64 > MAX_PIXELS {
            return false;
        }
        if self.w != w || self.h != h {
            self.w = w;
            self.h = h;
            self.rgba = vec![0; w as usize * h as usize * 4];
        }
        true
    }

    fn put(&mut self, rect: Rect, pixels: &[u8]) {
        let (w, h) = (rect.width as u32, rect.height as u32);
        if w == 0 || h == 0 || pixels.len() < w as usize * h as usize * 4 {
            return;
        }
        for row in 0..h {
            let y = rect.y as u32 + row;
            if y >= self.h {
                break;
            }
            let x0 = rect.x as u32;
            let n = w.min(self.w.saturating_sub(x0)) as usize;
            if n == 0 {
                continue;
            }
            let src = (row as usize * w as usize) * 4;
            let dst = (y as usize * self.w as usize + x0 as usize) * 4;
            self.rgba[dst..dst + n * 4].copy_from_slice(&pixels[src..src + n * 4]);
        }
    }

    fn copy_rect(&mut self, dst: Rect, src: Rect) {
        let (w, h) = (
            dst.width.min(src.width) as u32,
            dst.height.min(src.height) as u32,
        );
        if w == 0 || h == 0 {
            return;
        }
        let mut taken = vec![0u8; w as usize * h as usize * 4];
        for row in 0..h {
            let y = src.y as u32 + row;
            if y >= self.h {
                break;
            }
            let x0 = src.x as u32;
            let n = w.min(self.w.saturating_sub(x0)) as usize;
            if n == 0 {
                continue;
            }
            let from = (y as usize * self.w as usize + x0 as usize) * 4;
            let to = row as usize * w as usize * 4;
            taken[to..to + n * 4].copy_from_slice(&self.rgba[from..from + n * 4]);
        }
        self.put(
            Rect {
                width: w as u16,
                height: h as u16,
                ..dst
            },
            &taken,
        );
    }

    fn put_jpeg(&mut self, rect: Rect, jpeg: &[u8]) {
        let Ok(image) = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg) else {
            return;
        };
        let rgba = image.to_rgba8();
        if rgba.width() != rect.width as u32 || rgba.height() != rect.height as u32 {
            return;
        }
        self.put(rect, rgba.as_raw());
    }
}

/// Nearest-neighbour copy of `raster` into `slot`.
pub fn paint(c: &mut Canvas, area: Slot, raster: &Raster) {
    let placed = letterbox(area, raster.w, raster.h);
    let (w, h) = (placed.w.round() as u32, placed.h.round() as u32);
    if w == 0
        || h == 0
        || raster.w == 0
        || raster.h == 0
        || raster.rgba.len() < raster.w as usize * raster.h as usize * 4
    {
        return;
    }
    let mut layer = Canvas::filled(
        w,
        h,
        crate::color::Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        },
    );
    for py in 0..h {
        let sy = ((py as u64 * raster.h as u64) / h as u64).min(raster.h as u64 - 1) as u32;
        for px in 0..w {
            let sx = ((px as u64 * raster.w as u64) / w as u64).min(raster.w as u64 - 1) as u32;
            let from = (sy as usize * raster.w as usize + sx as usize) * 4;
            let to = (py as usize * w as usize + px as usize) * 4;
            layer.data[to..to + 4].copy_from_slice(&raster.rgba[from..from + 4]);
            layer.data[to + 3] = 255;
        }
    }
    c.lay(
        &layer,
        placed.x.round() as i32,
        placed.y.round() as i32,
        1.0,
    );
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Connecting,
    Live,
    Failed(String),
}

struct Shared {
    picture: Mutex<Option<Arc<Raster>>>,
    phase: Mutex<Phase>,
    generation: AtomicU64,
}

impl Shared {
    fn phase(&self) -> Phase {
        self.phase
            .lock()
            .map(|p| p.clone())
            .unwrap_or(Phase::Failed("stopped".into()))
    }

    fn set_phase(&self, phase: Phase) {
        if let Ok(mut slot) = self.phase.lock() {
            *slot = phase;
        }
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    fn publish(&self, raster: &Raster) {
        if let Ok(mut slot) = self.picture.lock() {
            *slot = Some(Arc::new(Raster {
                w: raster.w,
                h: raster.h,
                rgba: raster.rgba.clone(),
            }));
        }
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

enum Cmd {
    Pointer { x: u16, y: u16, buttons: u8 },
    Key { key: u32, down: bool },
    Stop,
}

/// One connection. Dropping it asks the worker to disconnect.
pub struct Link {
    shared: Arc<Shared>,
    tx: UnboundedSender<Cmd>,
}

impl Link {
    pub fn start(addr: String, password: String) -> Self {
        let shared = Arc::new(Shared {
            picture: Mutex::new(None),
            phase: Mutex::new(Phase::Connecting),
            generation: AtomicU64::new(1),
        });
        let (tx, rx) = mpsc::unbounded_channel();
        let worker = Arc::clone(&shared);
        let _ = std::thread::Builder::new()
            .name("vnc".into())
            .spawn(move || run(addr, password, worker, rx));
        Self { shared, tx }
    }

    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Relaxed)
    }

    pub fn phase(&self) -> Phase {
        self.shared.phase()
    }

    pub fn picture(&self) -> Option<Arc<Raster>> {
        self.shared
            .picture
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }

    pub fn pointer(&self, x: u16, y: u16, buttons: u8) {
        let _ = self.tx.send(Cmd::Pointer { x, y, buttons });
    }

    pub fn key(&self, key: u32, down: bool) {
        let _ = self.tx.send(Cmd::Key { key, down });
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Stop);
    }
}

/// An X11 keysym for a Latin-1 character, a space, return or backspace.
pub fn keysym(cap: crate::widgets::Cap, shifted: bool) -> Option<u32> {
    use crate::widgets::Cap;
    match cap {
        Cap::Char(ch) => {
            let ch = if shifted { ch.to_ascii_uppercase() } else { ch };
            let u = ch as u32;
            (u <= 0xff).then_some(u)
        }
        Cap::Space => Some(0x20),
        Cap::Enter => Some(0xff0d),
        Cap::Backspace => Some(0xff08),
        Cap::Shift | Cap::Show(_) => None,
    }
}

fn run(addr: String, password: String, shared: Arc<Shared>, rx: mpsc::UnboundedReceiver<Cmd>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            shared.set_phase(Phase::Failed(err.to_string()));
            return;
        }
    };
    rt.block_on(session(addr, password, shared, rx));
}

async fn session(
    addr: String,
    password: String,
    shared: Arc<Shared>,
    mut rx: mpsc::UnboundedReceiver<Cmd>,
) {
    let stream = match tokio::time::timeout(
        Duration::from_secs(8),
        tokio::net::TcpStream::connect(&addr),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(err)) => {
            shared.set_phase(Phase::Failed(err.to_string()));
            return;
        }
        Err(_) => {
            shared.set_phase(Phase::Failed(format!("{addr} timed out")));
            return;
        }
    };
    let client = match connect(stream, password).await {
        Ok(client) => client,
        Err(err) => {
            shared.set_phase(Phase::Failed(err.to_string()));
            return;
        }
    };
    let _ = client.input(X11Event::FullRefresh).await;
    shared.set_phase(Phase::Live);
    let mut raster = Raster::new();
    let mut dirty = false;
    let mut published = Instant::now();
    loop {
        tokio::select! {
            biased;
            cmd = rx.recv() => {
                match cmd {
                    None | Some(Cmd::Stop) => break,
                    Some(Cmd::Pointer { x, y, buttons }) => {
                        let _ = client.input(X11Event::PointerEvent(ClientMouseEvent { position_x: x, position_y: y, bottons: buttons })).await;
                    }
                    Some(Cmd::Key { key, down }) => {
                        let _ = client.input(X11Event::KeyEvent(ClientKeyEvent { keycode: key, down })).await;
                    }
                }
            }
            event = client.poll_event() => {
                match event {
                    Ok(Some(event)) => {
                        if apply(&mut raster, event) {
                            dirty = true;
                        }
                    }
                    Ok(None) => {
                        if dirty {
                            shared.publish(&raster);
                            dirty = false;
                            published = Instant::now();
                        }
                        tokio::time::sleep(Duration::from_millis(16)).await;
                    }
                    Err(err) => {
                        shared.set_phase(Phase::Failed(err.to_string()));
                        break;
                    }
                }
            }
        }
        if dirty && published.elapsed() >= Duration::from_millis(40) {
            shared.publish(&raster);
            dirty = false;
            published = Instant::now();
        }
        if published.elapsed() >= Duration::from_millis(80) {
            let _ = client.input(X11Event::Refresh).await;
            published = Instant::now();
        }
    }
    if dirty {
        shared.publish(&raster);
    }
    let _ = client.close().await;
}

async fn connect(
    stream: tokio::net::TcpStream,
    password: String,
) -> Result<vnc::VncClient, vnc::VncError> {
    VncConnector::new(stream)
        .set_auth_method(async move { Ok(password) })
        .add_encoding(VncEncoding::Tight)
        .add_encoding(VncEncoding::Zrle)
        .add_encoding(VncEncoding::CopyRect)
        .add_encoding(VncEncoding::Raw)
        .allow_shared(true)
        .set_pixel_format(PixelFormat::rgba())
        .build()?
        .try_start()
        .await?
        .finish()
}

/// Apply one decoded rectangle. `false` means the picture did not change.
fn apply(raster: &mut Raster, event: VncEvent) -> bool {
    match event {
        VncEvent::SetResolution(screen) => raster.resize(screen.width as u32, screen.height as u32),
        VncEvent::DesktopUpdate(update) => update
            .layout
            .is_some_and(|layout| raster.resize(layout.width as u32, layout.height as u32)),
        VncEvent::RawImage(rect, pixels) => {
            raster.put(rect, &pixels);
            true
        }
        VncEvent::Copy(dst, src) => {
            raster.copy_rect(dst, src);
            true
        }
        VncEvent::JpegImage(rect, jpeg) => {
            raster.put_jpeg(rect, &jpeg);
            true
        }
        VncEvent::SetPixelFormat(_)
        | VncEvent::SetCursor(_, _)
        | VncEvent::Bell
        | VncEvent::Text(_)
        | VncEvent::Error(_)
        | _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_address_does_not_connect() {
        assert_eq!(endpoint("  "), None);
        assert_eq!(endpoint("pc.local"), Some("pc.local:5900".into()));
        assert_eq!(endpoint("pc.local:5901"), Some("pc.local:5901".into()));
        assert_eq!(endpoint("127.0.0.1:5901"), Some("127.0.0.1:5901".into()));
    }

    #[test]
    fn a_click_lands_on_the_remote_pixel() {
        let area = Slot {
            x: 0.0,
            y: 100.0,
            w: 200.0,
            h: 100.0,
        };
        // The 400×100 picture fits the 200-wide slot at half scale, so it is 200×50
        // and sits 25px below the slot's top. A point on that top edge is remote y 0.
        assert_eq!(map_point(area, 400, 100, 50.0, 125.0), Some((100, 0)));
        assert_eq!(map_point(area, 400, 100, 50.0, 150.0), Some((100, 50)));
        assert_eq!(map_point(area, 400, 100, 10.0, 10.0), None);
    }

    #[test]
    fn raw_and_jpeg_rectangles_fill_the_frame() {
        let mut raster = Raster::new();
        assert!(apply(
            &mut raster,
            VncEvent::SetResolution(vnc::Screen {
                width: 2,
                height: 1
            })
        ));
        apply(
            &mut raster,
            VncEvent::RawImage(
                Rect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                vec![1, 2, 3, 255],
            ),
        );
        let jpeg = tiny_jpeg();
        apply(
            &mut raster,
            VncEvent::JpegImage(
                Rect {
                    x: 1,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                jpeg,
            ),
        );
        assert_eq!(&raster.rgba[0..4], &[1, 2, 3, 255]);
        assert!(
            raster.rgba[4] > 200 && raster.rgba[5] < 20 && raster.rgba[6] < 20,
            "jpeg red"
        );
    }

    #[test]
    fn a_closed_port_fails_the_session() {
        let link = Link::start("127.0.0.1:1".into(), String::new());
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            if matches!(link.phase(), Phase::Failed(_)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("still {:?}", link.phase());
    }

    fn tiny_jpeg() -> Vec<u8> {
        let mut bytes = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new(&mut bytes);
        use image::ImageEncoder;
        encoder
            .write_image(&[255, 0, 0], 1, 1, image::ExtendedColorType::Rgb8)
            .expect("jpeg");
        bytes
    }
}
