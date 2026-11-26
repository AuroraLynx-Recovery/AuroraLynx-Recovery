//! Cellular data inside the recovery: radio modules the product already
//! lists, modem helpers it installed on vendor or odm, then an address
//! the interface already has or DHCP gives.

use anyhow::{ensure, Context, Result};
use skyward_config::Mobile as Config;
use skyward_net::dhcp::{self, Lease};
use skyward_net::http::Servers;
use skyward_net::iface;
use skyward_net::mobile::{self, Mobile};
use skyward_net::reach::{self, Reach};
use skyward_net::wlan::Power;
use skyward_sys::worker::{self, Teller};
use std::fs;
use std::net::Ipv4Addr;
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const INTERFACE_WAIT: Duration = Duration::from_secs(45);
const RETRY: Duration = Duration::from_secs(10);

pub enum Command {
    On,
    Off,
}

enum Inbox {
    Command(Command),
    Address(u64, Result<Lease>),
    Reach(Reach),
    End,
}

enum Report {
    Mobile(Mobile),
}

pub struct Worker {
    inbox: Teller<Inbox>,
    worker: worker::Worker<Report>,
}

impl Worker {
    pub fn start(cfg: Config, modules: Vec<PathBuf>, libraries: Option<PathBuf>, servers: Servers) -> Result<Worker> {
        let (inbox, asked) = worker::telling().context("make the mobile thread's inbox")?;
        let back = inbox.clone();
        let worker = worker::Worker::start("mobile", move |tell| {
            let mut s = Station {
                cfg,
                modules,
                libraries,
                servers,
                state: Mobile::default(),
                children: Vec::new(),
                interface: None,
                asking: None,
                asked: 0,
                retry_at: None,
                back,
                tell,
            };
            s.run(&asked);
        })?;
        Ok(Worker { inbox, worker })
    }

    pub fn send(&self, c: Command) {
        self.inbox.tell(Inbox::Command(c));
    }

    pub fn fd(&self) -> RawFd {
        self.worker.fd()
    }

    pub fn told(&self) -> Option<Mobile> {
        self.worker.events().into_iter().filter_map(|r| match r {
            Report::Mobile(m) => Some(m),
        }).last()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.inbox.tell(Inbox::End);
    }
}

struct Station {
    cfg: Config,
    modules: Vec<PathBuf>,
    libraries: Option<PathBuf>,
    servers: Servers,
    state: Mobile,
    children: Vec<Child>,
    interface: Option<String>,
    asking: Option<(u64, Arc<AtomicBool>)>,
    asked: u64,
    retry_at: Option<Instant>,
    back: Teller<Inbox>,
    tell: Teller<Report>,
}

impl Station {
    fn run(&mut self, asked: &worker::Told<Inbox>) {
        loop {
            let mut fds = [libc::pollfd { fd: asked.fd(), events: libc::POLLIN, revents: 0 }];
            let timeout = self.retry_at.map_or(-1, |at| at.saturating_duration_since(Instant::now()).as_millis().min(60_000) as i32);
            if unsafe { libc::poll(fds.as_mut_ptr(), 1, timeout) } < 0 {
                let e = std::io::Error::last_os_error();
                assert_eq!(e.kind(), std::io::ErrorKind::Interrupted, "poll the mobile thread's inbox: {e}");
            }
            if self.retry_at.is_some_and(|at| Instant::now() >= at) {
                self.retry_at = None;
                if matches!(self.state.power, Power::On) && self.state.address.is_none() {
                    self.ask_address();
                }
            }
            for message in asked.take() {
                match message {
                    Inbox::Command(Command::On) => self.on(),
                    Inbox::Command(Command::Off) => self.off(),
                    Inbox::Address(n, answer) => self.addressed(n, answer),
                    Inbox::Reach(reach) => {
                        let _ = skyward_sys::kmsg(&format!("mobile {reach}"));
                        self.state.reach = Some(reach);
                        self.show();
                    }
                    Inbox::End => return self.off(),
                }
            }
        }
    }

    fn show(&self) {
        self.tell.tell(Report::Mobile(self.state.clone()));
    }

    fn on(&mut self) {
        if !matches!(self.state.power, Power::Off | Power::Failed(_)) {
            return;
        }
        self.state = Mobile { power: Power::Starting, ..Mobile::default() };
        self.show();
        match self.bring_up() {
            Ok(()) => {
                self.state.power = Power::On;
                self.show();
                self.ask_address();
            }
            Err(e) => {
                let _ = skyward_sys::kmsg(&format!("mobile did not come up: {e:#}"));
                self.off();
                self.state.power = Power::Failed(format!("{e:#}"));
                self.show();
            }
        }
    }

    fn off(&mut self) {
        if let Some((_, stop)) = self.asking.take() {
            stop.store(true, Ordering::Relaxed);
        }
        self.retry_at = None;
        if let Some(iface) = &self.interface {
            let _ = iface::unconfigure(iface);
        }
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.children.clear();
        self.interface = None;
        self.state = Mobile::default();
        self.show();
    }

    fn bring_up(&mut self) -> Result<()> {
        if !self.cfg.modules.is_empty() {
            for path in skyward_modules::load(&self.modules, &self.cfg.modules)? {
                skyward_sys::kmsg(&format!("loaded {}", path.display()))?;
            }
        }
        let mut path: Vec<String> = self.cfg.libraries.clone();
        if let Some(dir) = &self.libraries {
            path.push(dir.to_string_lossy().into_owned());
        }
        let joined = path.join(":");
        for helper in &self.cfg.helpers {
            if !Path::new(helper).is_file() {
                let _ = skyward_sys::kmsg(&format!("{helper} is not on this phone"));
                continue;
            }
            match Process::new(helper)
                .env("LD_LIBRARY_PATH", &joined)
                .current_dir("/")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => {
                    let _ = skyward_sys::kmsg(&format!("started {helper}"));
                    self.children.push(child);
                }
                Err(e) => {
                    let _ = skyward_sys::kmsg(&format!("{helper} did not start: {e}"));
                }
            }
        }
        let prefixes: Vec<&str> = self.cfg.prefixes.iter().map(String::as_str).collect();
        let until = Instant::now() + INTERFACE_WAIT;
        loop {
            if let Some(name) = list_interface(&prefixes) {
                iface::set_up(&name, true)?;
                self.interface = Some(name.clone());
                self.state.interface = Some(name);
                return Ok(());
            }
            ensure!(Instant::now() < until, "no cellular interface in {} seconds", INTERFACE_WAIT.as_secs());
            thread::sleep(Duration::from_millis(250));
        }
    }

    fn ask_address(&mut self) {
        let Some(interface) = self.interface.clone() else { return };
        if let Ok(Some(address)) = iface::current_ipv4(&interface) {
            let _ = skyward_sys::kmsg(&format!("{interface} already has {address}"));
            self.state.address = Some(address);
            self.state.power = Power::On;
            *self.servers.lock().expect("the DNS servers are not held by a thread that panicked") = vec![Ipv4Addr::new(8, 8, 8, 8)];
            self.show();
            self.probe(vec![Ipv4Addr::new(8, 8, 8, 8)]);
            return;
        }
        if let Some((_, stop)) = self.asking.take() {
            stop.store(true, Ordering::Relaxed);
        }
        self.asked += 1;
        let n = self.asked;
        let stop = Arc::new(AtomicBool::new(false));
        self.asking = Some((n, stop.clone()));
        let mac = iface::hardware_address(&interface).unwrap_or([0; 6]);
        let back = self.back.clone();
        thread::spawn(move || {
            let answer = dhcp::obtain(&interface, mac, &stop);
            back.tell(Inbox::Address(n, answer));
        });
    }

    fn addressed(&mut self, n: u64, answer: Result<Lease>) {
        if self.asking.as_ref().map(|(m, _)| *m) != Some(n) {
            return;
        }
        self.asking = None;
        let Some(interface) = self.interface.clone() else { return };
        let lease = match answer {
            Ok(lease) => lease,
            Err(e) if e.is::<skyward_net::Stopped>() => return,
            Err(e) => {
                let _ = skyward_sys::kmsg(&format!("mobile has no DHCP address: {e:#}"));
                if let Ok(Some(address)) = iface::current_ipv4(&interface) {
                    self.state.address = Some(address);
                    self.show();
                    return;
                }
                self.retry_at = Some(Instant::now() + RETRY);
                return;
            }
        };
        if let Err(e) = iface::configure(&interface, lease.address, lease.mask, lease.router) {
            let _ = skyward_sys::kmsg(&format!("{e:#}"));
            self.retry_at = Some(Instant::now() + RETRY);
            return;
        }
        let _ = skyward_sys::kmsg(&format!(
            "{interface} has {} ({}) through {:?}",
            lease.address, lease.mask, lease.router
        ));
        *self.servers.lock().expect("the DNS servers are not held by a thread that panicked") = lease.dns.clone();
        self.state = Mobile::from_lease(&lease, interface);
        self.show();
        self.probe(lease.dns);
    }

    fn probe(&self, dns: Vec<Ipv4Addr>) {
        let page = if self.cfg.probe.is_empty() {
            "http://connectivitycheck.gstatic.com/generate_204".to_owned()
        } else {
            self.cfg.probe.clone()
        };
        let back = self.back.clone();
        thread::spawn(move || {
            back.tell(Inbox::Reach(reach::probe(&page, &dns)));
        });
    }
}

fn list_interface(prefixes: &[&str]) -> Option<String> {
    let Ok(dir) = fs::read_dir("/sys/class/net") else { return None };
    let names: Vec<String> = dir.filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned())).collect();
    mobile::pick_interface(names.iter().map(String::as_str), prefixes)
}
