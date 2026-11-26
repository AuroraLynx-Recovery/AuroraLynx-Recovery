//! Render the moments AuroraLynx moves, frame by frame, as PNGs to look at:
//! a card held, a switch toggled, a slider dragged and an edge swipe going
//! back. Run with the directory holding MiSansVF.ttf and
//! KirkMono-Regular.ttf, and the directory to write the frames into:
//!
//! `cargo run --example motion_shots -- <fonts> <out>`

use skyward_info::{Battery, Charge, Cpu, DeviceInfo, Storage};
use skyward_ui::app::{Action, App, Data, Screen, Touch};
use skyward_ui::theme::UiStyle;
use skyward_ui::{Canvas, Fonts};
use skyward_vab::{Slot, SnapshotState};
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

const W: u32 = 1116;
const H: u32 = 2480;
const PPD: f32 = 2.875;
const FRAME_US: u64 = 8333;

struct Shooter<'a> {
    app: App,
    f: &'a Fonts,
    i: &'a DeviceInfo,
    out: Canvas,
    us: u64,
    dir: PathBuf,
    shot: u32,
    draw_ms: Vec<f64>,
}

impl<'a> Shooter<'a> {
    fn new(f: &'a Fonts, i: &'a DeviceInfo, dir: PathBuf, screen: Screen) -> Self {
        let mut app = App::new(screen, W, H, PPD);
        app.set_screen_corners(0.0);
        app.state.style = UiStyle::MaterialExpressive;
        app.state.data = Data::Open;
        let mut s = Shooter { app, f, i, out: Canvas::new(W, H), us: 0, dir, shot: 0, draw_ms: Vec::new() };
        s.run(200);
        if std::env::var_os("MOTION_HITS").is_some() {
            let frame = s.app.frame(s.f, s.i);
            for h in &frame.hits {
                println!("hit {:?} {:?}", h.action, h.rect);
            }
        }
        s
    }

    fn now(&self) -> u64 {
        self.us / 1000
    }

    fn frame(&mut self) {
        self.us += FRAME_US;
        let now = self.now();
        self.app.tick(now);
        if self.app.take_dirty() {
            let started = std::time::Instant::now();
            self.app.compose(&mut self.out, self.f, self.i, "12:00", now);
            self.draw_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        }
    }

    fn run(&mut self, ms: u64) {
        let end = self.us + ms * 1000;
        while self.us < end {
            self.frame();
        }
    }

    fn save(&mut self, what: &str) {
        let name = format!("{:02}-{}-{:04}ms.png", self.shot, what, self.now());
        self.shot += 1;
        let path = self.dir.join(name);
        write_png(&path, &self.out).expect("write a frame");
    }

    fn touch(&mut self, ev: Touch) {
        let now = self.now();
        self.app.touch(ev, self.f, now);
    }

    fn place_of(&mut self, action: &Action) -> (f32, f32) {
        let screen = self.app.screen();
        let frame = self.app.frame(self.f, self.i);
        frame.hits.iter().find(|h| &h.action == action).unwrap_or_else(|| panic!("{action:?} shows on {screen:?}")).rect.centre()
    }

    fn first_where(&mut self, pred: impl Fn(&Action) -> bool) -> (Action, skyward_ui::geom::Rect) {
        let screen = self.app.screen();
        let frame = self.app.frame(self.f, self.i);
        let hit = frame.hits.iter().find(|h| pred(&h.action)).unwrap_or_else(|| panic!("nothing like that shows on {screen:?}"));
        (hit.action.clone(), hit.rect)
    }

    fn tap_tab(&mut self, screen: Screen) {
        let frame = self.app.frame(self.f, self.i);
        let dock = frame.dock.as_ref().expect("Material tabs have a dock");
        let hit = dock.hits.iter().find(|h| h.action == Action::SelectTab(screen)).expect("the tab is offered");
        let top = H as f32 - dock.canvas.h as f32;
        let (x, y) = (hit.rect.centre().0, top + hit.rect.centre().1);
        self.touch(Touch::Down(x, y));
        self.frame();
        self.touch(Touch::Up);
    }

    fn tap(&mut self, action: &Action) {
        let (x, y) = self.place_of(action);
        self.touch(Touch::Down(x, y));
        self.frame();
        self.touch(Touch::Up);
    }

    fn open(&mut self, screen: Screen) {
        self.tap(&Action::Open(screen));
        self.run(1200);
    }
}

fn write_png(path: &Path, c: &Canvas) -> std::io::Result<()> {
    let mut encoder = png::Encoder::new(BufWriter::new(File::create(path)?), c.w, c.h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&c.data).map_err(std::io::Error::other)?;
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let fonts = args.get(1).expect("the directory holding the fonts");
    let out = PathBuf::from(args.get(2).expect("the directory to write the frames into"));
    std::fs::create_dir_all(&out).expect("make the output directory");
    let f = Fonts::load(format!("{fonts}/MiSansVF.ttf"), 0, format!("{fonts}/KirkMono-Regular.ttf")).expect("load the fonts");
    let i = info();
    card_held(&f, &i, out.join("press"));
    switch_toggled(&f, &i, out.join("switch"));
    slider_dragged(&f, &i, out.join("slider"));
    edge_swiped(&f, &i, out.join("back"));
    edge_cancelled(&f, &i, out.join("back-cancel"));
}

/// A row of the home page held down, and let go.
fn card_held(f: &Fonts, i: &DeviceInfo, dir: PathBuf) {
    std::fs::create_dir_all(&dir).expect("make the directory");
    let mut s = Shooter::new(f, i, dir, Screen::Home);
    s.save("rest");
    let (x, y) = s.place_of(&Action::Open(Screen::About));
    s.touch(Touch::Down(x, y));
    for _ in 0..8 {
        s.frame();
        s.save("down");
    }
    s.run(160);
    s.save("held");
    s.touch(Touch::Up);
    for _ in 0..10 {
        s.frame();
        s.save("up");
    }
    s.run(400);
    s.save("rest-again");
}

/// A switch tapped on the settings page.
fn switch_toggled(f: &Fonts, i: &DeviceInfo, dir: PathBuf) {
    std::fs::create_dir_all(&dir).expect("make the directory");
    let mut s = Shooter::new(f, i, dir, Screen::Settings);
    let (action, _) = s.first_where(|a| matches!(a, Action::Toggle(_)));
    s.save("rest");
    let (x, y) = s.place_of(&action);
    s.touch(Touch::Down(x, y));
    for _ in 0..6 {
        s.frame();
        s.save("down");
    }
    s.touch(Touch::Up);
    for _ in 0..40 {
        s.frame();
        s.save("travel");
    }
    s.run(400);
    s.save("rest-again");
}

/// The brightness slider dragged across its track.
fn slider_dragged(f: &Fonts, i: &DeviceInfo, dir: PathBuf) {
    std::fs::create_dir_all(&dir).expect("make the directory");
    let mut s = Shooter::new(f, i, dir, Screen::Home);
    s.tap_tab(Screen::Settings);
    s.run(900);
    s.open(Screen::DisplayPreferences);
    let (action, rect) = s.first_where(|a| matches!(a, Action::Slider(_)));
    s.save("rest");
    let (x, y) = rect.centre();
    s.touch(Touch::Down(x, y));
    for k in 1..=24 {
        let step = rect.w * 0.5 * k as f32 / 24.0;
        s.touch(Touch::Move(x + step, y));
        s.frame();
        s.save("drag");
    }
    s.touch(Touch::Up);
    for _ in 0..10 {
        s.frame();
        s.save("up");
    }
    let _ = action;
    s.run(400);
    s.save("rest-again");
}

/// A finger in from the left edge, held, then let go past the threshold.
fn edge_swiped(f: &Fonts, i: &DeviceInfo, dir: PathBuf) {
    std::fs::create_dir_all(&dir).expect("make the directory");
    let mut s = Shooter::new(f, i, dir, Screen::Home);
    s.tap_tab(Screen::Settings);
    s.run(900);
    s.open(Screen::DisplayPreferences);
    s.save("rest");
    let y = H as f32 / 2.0;
    s.touch(Touch::Down(4.0, y));
    s.frame();
    for step in [0.08, 0.16, 0.24, 0.32, 0.4, 0.48, 0.56] {
        s.touch(Touch::Move(W as f32 * step, y));
        s.frame();
        s.save("drag");
    }
    s.touch(Touch::Up);
    for _ in 0..24 {
        s.frame();
        s.save("settle");
    }
    s.run(600);
    s.save("done");
}

/// A swipe in that stops short, and springs back.
fn edge_cancelled(f: &Fonts, i: &DeviceInfo, dir: PathBuf) {
    std::fs::create_dir_all(&dir).expect("make the directory");
    let mut s = Shooter::new(f, i, dir, Screen::Home);
    s.tap_tab(Screen::Settings);
    s.run(900);
    s.open(Screen::DisplayPreferences);
    s.save("rest");
    let y = H as f32 / 2.0;
    s.touch(Touch::Down(4.0, y));
    s.frame();
    for step in [0.06, 0.12, 0.18, 0.24] {
        s.touch(Touch::Move(W as f32 * step, y));
        s.frame();
        s.save("drag");
    }
    s.touch(Touch::Up);
    for _ in 0..24 {
        s.frame();
        s.save("spring");
    }
    s.run(600);
    s.save("done");
    let mut ms = std::mem::take(&mut s.draw_ms);
    ms.sort_by(f64::total_cmp);
    if !ms.is_empty() {
        let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
        println!("back frames: {} drawn, median {:.2} ms, 90% {:.2}, slowest {:.2}", ms.len(), at(0.5), at(0.9), at(1.0));
    }
}

fn info() -> DeviceInfo {
    DeviceInfo {
        model: "Nubia Z60 Ultra".into(),
        codename: "cerro".into(),
        soc: None,
        cpu: Cpu { cores: 8, max_khz: Some(3_200_000) },
        ram_kib: 15_556_972,
        panel: Some((W, H)),
        screen: Some(6.8),
        os_version: None,
        android: None,
        locale: None,
        time_zone: None,
        security_patch: None,
        kernel: "6.1.25".into(),
        storage: Storage { total: 496_550_785_024, free: None },
        battery: Some(Battery {
            level: 64,
            charge: Charge::Charging,
            temp: Some(352),
            health: None,
            cycles: None,
            design: None,
            full: None,
            voltage: None,
            quick: false,
            plugged: true,
        }),
        slot: Some(Slot::B),
        snapshot: SnapshotState::None,
        boot: skyward_info::Boot { unlocked: None, verified: None },
    }
}
