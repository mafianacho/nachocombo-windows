//! Reads the system: performance counters, process tables, adapters and the battery. No driver, no administrator.
use std::collections::HashMap;
use windows::core::*;
use windows::Win32::Devices::DeviceAndDriverInstallation::*;
use windows::Win32::Foundation::*;
use windows::Win32::NetworkManagement::IpHelper::*;
use windows::Win32::NetworkManagement::Ndis::*;
use windows::Win32::Networking::WinSock::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Performance::*;
use windows::Win32::System::Power::*;
use windows::Win32::System::ProcessStatus::*;
use windows::Win32::System::SystemInformation::*;
use windows::Win32::System::Threading::*;

#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub cpu_user: f64,
    pub cpu_system: f64,
    pub cores: Vec<f64>,
    pub efficiency: Vec<bool>,
    pub queue: Option<f64>,
    pub mhz: Option<f64>,
    pub uptime: u64,
    pub mem_total: f64,
    pub mem_used: f64,
    pub mem_load: f64,
    pub mem_cache: f64,
    pub mem_kernel: f64,
    pub commit_used: f64,
    pub commit_limit: f64,
    pub page_in: Option<f64>,
    pub page_out: Option<f64>,
    pub disk_read: f64,
    pub disk_write: f64,
    pub volumes: Vec<Volume>,
    pub net_down: f64,
    pub net_up: f64,
    pub gpu: Option<f64>,
    pub gpu_mem: Option<f64>,
    pub temps: Vec<(String, f64)>,
    pub battery: Option<Battery>,
}

#[derive(Clone, Debug, Default)]
pub struct Volume { pub root: String, pub label: String, pub total: f64, pub free: f64 }

#[derive(Clone, Debug, Default)]
pub struct Battery {
    pub percent: f64,
    pub on_ac: bool,
    pub charging: bool,
    pub seconds_left: Option<u32>,
    pub cycles: Option<u32>,
    pub design: Option<u32>,
    pub full: Option<u32>,
    pub rate_mw: Option<i32>,
    pub voltage_mv: Option<u32>,
}

impl Battery {
    pub fn health(&self) -> Option<f64> { match (self.design, self.full) { (Some(d), Some(f)) if d > 0 => Some((f as f64 / d as f64 * 100.0).min(100.0)), _ => None } }
    pub fn state(&self) -> &'static str { if self.charging { "Charging" } else if self.on_ac { if self.percent >= 99.0 { "Charged" } else { "Not Charging" } } else { "On Battery" } }
}

#[derive(Clone, Debug, Default)]
pub struct Proc { pub name: String, pub cpu: f64, pub mem: f64, pub read: f64, pub write: f64 }

#[derive(Clone, Debug, Default)]
pub struct Adapter { pub name: String, pub addresses: Vec<String>, pub speed: u64, pub up: bool }

struct Counter(PDH_HCOUNTER);
const NOCAP: PDH_FMT = PDH_FMT(0x8000);

pub struct Sampler {
    query: PDH_HQUERY,
    counters: HashMap<&'static str, Counter>,
    last_sys: Option<(u64, u64, u64)>,
    last_cores: Vec<(i64, i64, i64)>,
    efficiency: Vec<bool>,
    last_net: Option<(u64, u64, std::time::Instant)>,
    last_procs: HashMap<u32, (u64, u64, u64, std::time::Instant)>,
    cores_ok: bool,
    ticks: u32,
    volumes: Vec<Volume>,
    battery_path: Option<Vec<u16>>,
    battery_static: Option<(Option<u32>, Option<u32>, Option<u32>)>,
}

fn ft(f: FILETIME) -> u64 { (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64 }

const COUNTERS: [(&str, &str); 10] = [
    ("read", "\\PhysicalDisk(_Total)\\Disk Read Bytes/sec"),
    ("write", "\\PhysicalDisk(_Total)\\Disk Write Bytes/sec"),
    ("queue", "\\System\\Processor Queue Length"),
    ("pagein", "\\Memory\\Pages Input/sec"),
    ("pageout", "\\Memory\\Pages Output/sec"),
    ("freq", "\\Processor Information(_Total)\\Processor Frequency"),
    ("perf", "\\Processor Information(_Total)\\% Processor Performance"),
    ("gpu", "\\GPU Engine(*engtype_3D)\\Utilization Percentage"),
    ("gpumem", "\\GPU Adapter Memory(*)\\Dedicated Usage"),
    ("thermal", "\\Thermal Zone Information(*)\\High Precision Temperature"),
];

impl Sampler {
    pub fn new() -> Sampler {
        let mut s = Sampler { query: PDH_HQUERY::default(), counters: HashMap::new(), last_sys: None, last_cores: vec![], efficiency: efficiency_map(), last_net: None, last_procs: HashMap::new(), cores_ok: true, ticks: 0, volumes: vec![], battery_path: battery_device(), battery_static: None };
        unsafe {
            let mut q = PDH_HQUERY::default();
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut q) == 0 {
                s.query = q;
                for (key, path) in COUNTERS {
                    let mut c = PDH_HCOUNTER::default();
                    // English names, so the counters work on a Korean or any other Windows.
                    if PdhAddEnglishCounterW(q, &HSTRING::from(path), 0, &mut c) == 0 { s.counters.insert(key, Counter(c)); }
                }
                let _ = PdhCollectQueryData(q);
            }
        }
        s
    }

    fn value(&self, key: &str) -> Option<f64> {
        let c = self.counters.get(key)?;
        unsafe {
            let mut v = PDH_FMT_COUNTERVALUE::default();
            if PdhGetFormattedCounterValue(c.0, PDH_FMT(PDH_FMT_DOUBLE.0 | NOCAP.0), None, &mut v) == 0 && v.CStatus == 0 { Some(v.Anonymous.doubleValue) } else { None }
        }
    }

    /// Every instance of a wildcard counter, as (instance name, value).
    fn values(&self, key: &str) -> Vec<(String, f64)> {
        let Some(c) = self.counters.get(key) else { return vec![] };
        unsafe {
            let (mut size, mut count) = (0u32, 0u32);
            let _ = PdhGetFormattedCounterArrayW(c.0, PDH_FMT(PDH_FMT_DOUBLE.0 | NOCAP.0), &mut size, &mut count, None);
            if size == 0 { return vec![]; }
            let mut buf = vec![0u64; size as usize / 8 + 1];
            let p = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(c.0, PDH_FMT(PDH_FMT_DOUBLE.0 | NOCAP.0), &mut size, &mut count, Some(p)) != 0 { return vec![]; }
            std::slice::from_raw_parts(p, count as usize).iter().filter(|i| i.FmtValue.CStatus == 0).map(|i| (i.szName.to_string().unwrap_or_default(), i.FmtValue.Anonymous.doubleValue)).collect()
        }
    }

    pub fn sample(&mut self) -> Sample {
        let mut o = Sample { efficiency: self.efficiency.clone(), ..Default::default() };
        unsafe {
            let _ = PdhCollectQueryData(self.query);
            let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
            if GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).is_ok() {
                let now = (ft(idle), ft(kernel), ft(user));
                if let Some(l) = self.last_sys {
                    let (di, dk, du) = ((now.0 - l.0) as f64, (now.1 - l.1) as f64, (now.2 - l.2) as f64);
                    let total = dk + du;
                    if total > 0.0 {
                        o.cpu_user = du / total * 100.0;
                        o.cpu_system = ((dk - di).max(0.0)) / total * 100.0;
                    }
                }
                self.last_sys = Some(now);
            }
            o.cores = self.core_usage();
            o.uptime = windows::Win32::System::SystemInformation::GetTickCount64() / 1000;
            let mut mem = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
            if GlobalMemoryStatusEx(&mut mem).is_ok() {
                o.mem_total = mem.ullTotalPhys as f64;
                o.mem_used = (mem.ullTotalPhys - mem.ullAvailPhys) as f64;
                o.mem_load = o.mem_used / o.mem_total.max(1.0) * 100.0;
                o.commit_limit = mem.ullTotalPageFile as f64;
                o.commit_used = (mem.ullTotalPageFile - mem.ullAvailPageFile) as f64;
            }
            let mut pi = PERFORMANCE_INFORMATION { cb: std::mem::size_of::<PERFORMANCE_INFORMATION>() as u32, ..Default::default() };
            if GetPerformanceInfo(&mut pi, pi.cb).is_ok() {
                o.mem_cache = pi.SystemCache as f64 * pi.PageSize as f64;
                o.mem_kernel = pi.KernelTotal as f64 * pi.PageSize as f64;
            }
        }
        o.disk_read = self.value("read").unwrap_or(0.0);
        o.disk_write = self.value("write").unwrap_or(0.0);
        o.queue = self.value("queue");
        o.page_in = self.value("pagein");
        o.page_out = self.value("pageout");
        o.mhz = match (self.value("freq"), self.value("perf")) { (Some(f), Some(p)) => Some(f * p / 100.0), (f, _) => f };
        let gpus = self.values("gpu");
        // Several engines per process: the busiest adapter engine is what Task Manager shows.
        if !self.counters.contains_key("gpu") { o.gpu = None; } else { o.gpu = Some(gpus.iter().map(|x| x.1).sum::<f64>().min(100.0)); }
        let gm = self.values("gpumem");
        if !gm.is_empty() { o.gpu_mem = Some(gm.iter().map(|x| x.1).sum()); }
        // Tenths of a kelvin. Zones below freezing are firmware placeholders.
        o.temps = self.values("thermal").into_iter().map(|(n, v)| (n.rsplit('.').next().unwrap_or("Zone").replace("TZ", "Zone ").replace("_", ""), v / 10.0 - 273.15)).filter(|t| t.1 > 1.0 && t.1 < 150.0).collect();
        let (down, up) = self.net_rate();
        o.net_down = down;
        o.net_up = up;
        if self.ticks % 30 == 0 { self.volumes = read_volumes(); }
        o.volumes = self.volumes.clone();
        self.ticks = self.ticks.wrapping_add(1);
        o.battery = self.battery();
        o
    }

    fn core_usage(&mut self) -> Vec<f64> {
        if !self.cores_ok { return vec![]; }
        // Per-core times have no documented Win32 call; this one has been stable since Windows XP.
        #[repr(C)]
        #[derive(Clone, Copy, Default)]
        struct Info { idle: i64, kernel: i64, user: i64, dpc: i64, interrupt: i64, count: u32 }
        #[link(name = "ntdll")]
        extern "system" { fn NtQuerySystemInformation(class: u32, info: *mut core::ffi::c_void, len: u32, ret: *mut u32) -> i32; }
        let n = unsafe { GetActiveProcessorCount(0xFFFF) as usize };
        let mut buf = vec![Info::default(); n.max(1)];
        let mut ret = 0u32;
        let st = unsafe { NtQuerySystemInformation(8, buf.as_mut_ptr() as *mut _, (buf.len() * std::mem::size_of::<Info>()) as u32, &mut ret) };
        if st != 0 { self.cores_ok = false; return vec![]; }
        let now: Vec<(i64, i64, i64)> = buf.iter().map(|i| (i.idle, i.kernel, i.user)).collect();
        let out = if self.last_cores.len() == now.len() {
            now.iter().zip(&self.last_cores).map(|(a, b)| {
                let total = (a.1 - b.1 + a.2 - b.2) as f64;
                if total <= 0.0 { 0.0 } else { (1.0 - (a.0 - b.0) as f64 / total).clamp(0.0, 1.0) * 100.0 }
            }).collect()
        } else { vec![0.0; now.len()] };
        self.last_cores = now;
        out
    }

    fn net_rate(&mut self) -> (f64, f64) {
        let (mut rx, mut tx) = (0u64, 0u64);
        unsafe {
            let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
            if GetIfTable2(&mut table).is_err() { return (0.0, 0.0); }
            for r in std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize) {
                // Physical adapters only: filters and tunnels would count the same bytes twice.
                if r.InterfaceAndOperStatusFlags._bitfield & 1 != 0 && r.OperStatus == IfOperStatusUp && r.Type != IF_TYPE_SOFTWARE_LOOPBACK {
                    rx += r.InOctets;
                    tx += r.OutOctets;
                }
            }
            FreeMibTable(table as *const _);
        }
        let now = std::time::Instant::now();
        let out = match self.last_net {
            Some((a, b, t)) => { let dt = now.duration_since(t).as_secs_f64().max(0.1); (rx.saturating_sub(a) as f64 / dt, tx.saturating_sub(b) as f64 / dt) }
            None => (0.0, 0.0),
        };
        self.last_net = Some((rx, tx, now));
        out
    }

    fn battery(&mut self) -> Option<Battery> {
        unsafe {
            let mut st = SYSTEM_POWER_STATUS::default();
            GetSystemPowerStatus(&mut st).ok()?;
            if st.BatteryFlag & 128 != 0 || st.BatteryFlag == 255 { return None; }
            let on_ac = st.ACLineStatus == 1;
            let mut b = Battery { percent: st.BatteryLifePercent.min(100) as f64, on_ac, charging: st.BatteryFlag & 8 != 0, seconds_left: (st.BatteryLifeTime != u32::MAX && !on_ac).then_some(st.BatteryLifeTime), ..Default::default() };
            if let Some(path) = &self.battery_path { read_battery(path, &mut b, &mut self.battery_static); }
            Some(b)
        }
    }

    /// What each process did since the last call; only while a panel shows it.
    pub fn processes(&mut self, cores: usize) -> Vec<Proc> {
        let mut out = vec![];
        let mut seen = HashMap::new();
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
            let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let now = std::time::Instant::now();
            if Process32FirstW(snap, &mut e).is_ok() {
                loop {
                    let pid = e.th32ProcessID;
                    if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                        let (mut c, mut x, mut k, mut u) = Default::default();
                        let mut p = Proc { name: String::from_utf16_lossy(&e.szExeFile[..e.szExeFile.iter().position(|c| *c == 0).unwrap_or(0)]).trim_end_matches(".exe").to_string(), ..Default::default() };
                        if GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u).is_ok() {
                            let cpu = ft(k) + ft(u);
                            let mut io = IO_COUNTERS::default();
                            let _ = GetProcessIoCounters(h, &mut io);
                            let mut mem = PROCESS_MEMORY_COUNTERS::default();
                            if GetProcessMemoryInfo(h, &mut mem, std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32).is_ok() { p.mem = mem.WorkingSetSize as f64; }
                            if let Some((c0, r0, w0, t0)) = self.last_procs.get(&pid) {
                                let dt = now.duration_since(*t0).as_secs_f64().max(0.1);
                                p.cpu = (cpu.saturating_sub(*c0)) as f64 / 1e7 / dt / cores.max(1) as f64 * 100.0;
                                p.read = io.ReadTransferCount.saturating_sub(*r0) as f64 / dt;
                                p.write = io.WriteTransferCount.saturating_sub(*w0) as f64 / dt;
                            }
                            seen.insert(pid, (cpu, io.ReadTransferCount, io.WriteTransferCount, now));
                        }
                        let _ = CloseHandle(h);
                        out.push(p);
                    }
                    if Process32NextW(snap, &mut e).is_err() { break; }
                }
            }
            let _ = CloseHandle(snap);
        }
        self.last_procs = seen;
        // Helpers of one app count as that app.
        let mut merged: HashMap<String, Proc> = HashMap::new();
        for p in out {
            let e = merged.entry(p.name.clone()).or_insert(Proc { name: p.name.clone(), ..Default::default() });
            e.cpu += p.cpu; e.mem += p.mem; e.read += p.read; e.write += p.write;
        }
        merged.into_values().collect()
    }
}

impl Drop for Sampler {
    fn drop(&mut self) { unsafe { if !self.query.0.is_null() { let _ = PdhCloseQuery(self.query); } } }
}

/// A program's file name from its process id, if this account may read it.
pub fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n).is_ok();
        let _ = CloseHandle(h);
        if !ok { return None; }
        let full = String::from_utf16_lossy(&buf[..n as usize]);
        full.rsplit('\\').next().map(|s| s.trim_end_matches(".exe").to_string())
    }
}

fn efficiency_map() -> Vec<bool> {
    unsafe {
        let mut len = 0u32;
        let _ = GetLogicalProcessorInformationEx(RelationProcessorCore, None, &mut len);
        if len == 0 { return vec![]; }
        let mut buf = vec![0u8; len as usize];
        if GetLogicalProcessorInformationEx(RelationProcessorCore, Some(buf.as_mut_ptr() as *mut _), &mut len).is_err() { return vec![]; }
        let mut classes: Vec<(usize, u8)> = vec![];
        let mut off = 0usize;
        while off < len as usize {
            let info = &*(buf.as_ptr().add(off) as *const SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX);
            let core = &info.Anonymous.Processor;
            let group = &core.GroupMask[0];
            for bit in 0..64 { if group.Mask >> bit & 1 == 1 { classes.push((group.Group as usize * 64 + bit, core.EfficiencyClass)); } }
            off += info.Size as usize;
        }
        classes.sort();
        let top = classes.iter().map(|c| c.1).max().unwrap_or(0);
        // Class 0 is the efficient kind only on chips that have two kinds.
        classes.iter().map(|c| top > 0 && c.1 < top).collect()
    }
}

pub fn read_volumes() -> Vec<Volume> {
    let mut out = vec![];
    unsafe {
        let mask = GetLogicalDrives();
        for i in 0..26 {
            if mask >> i & 1 == 0 { continue; }
            let root = format!("{}:\\", (b'A' + i as u8) as char);
            let w = HSTRING::from(&root);
            if GetDriveTypeW(&w) != 3 { continue; } // DRIVE_FIXED: asking a network or card drive can block.
            let (mut free, mut total) = (0u64, 0u64);
            if GetDiskFreeSpaceExW(&w, None, Some(&mut total), Some(&mut free)).is_err() { continue; }
            let mut name = [0u16; 64];
            let _ = GetVolumeInformationW(&w, Some(&mut name), None, None, None, None);
            let n = name.iter().position(|c| *c == 0).unwrap_or(0);
            out.push(Volume { root, label: String::from_utf16_lossy(&name[..n]), total: total as f64, free: free as f64 });
        }
    }
    out
}

pub fn adapters() -> Vec<Adapter> {
    let mut out = vec![];
    unsafe {
        let mut size = 16384u32;
        let mut buf = vec![0u64; size as usize / 8];
        let p = buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
        if GetAdaptersAddresses(0, GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER, None, Some(p), &mut size) != 0 { return out; }
        let mut cur = p;
        while !cur.is_null() {
            let a = &*cur;
            if a.IfType != IF_TYPE_SOFTWARE_LOOPBACK {
                let mut addresses = vec![];
                let mut u = a.FirstUnicastAddress;
                while !u.is_null() {
                    let sa = (*u).Address.lpSockaddr;
                    if !sa.is_null() && (*sa).sa_family == AF_INET {
                        let v4 = &*(sa as *const SOCKADDR_IN);
                        let b = v4.sin_addr.S_un.S_un_b;
                        addresses.push(format!("{}.{}.{}.{}", b.s_b1, b.s_b2, b.s_b3, b.s_b4));
                    }
                    u = (*u).Next;
                }
                out.push(Adapter { name: a.FriendlyName.to_string().unwrap_or_default(), addresses, speed: a.ReceiveLinkSpeed, up: a.OperStatus == IfOperStatusUp });
            }
            cur = a.Next;
        }
    }
    out.sort_by_key(|a| !a.up);
    out
}

/// The public address. Called only on request; it is the one thing here that leaves the PC.
pub fn lookup_public_ip() -> Option<String> {
    use windows::Win32::Networking::WinHttp::*;
    unsafe {
        let s = WinHttpOpen(w!("NachoCombo"), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0);
        if s.is_null() { return None; }
        let c = WinHttpConnect(s, w!("www.cloudflare.com"), 443, 0);
        let r = if c.is_null() { std::ptr::null_mut() } else { WinHttpOpenRequest(c, w!("GET"), w!("/cdn-cgi/trace"), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE) };
        let mut text = Vec::new();
        if !r.is_null() && WinHttpSendRequest(r, None, None, 0, 0, 0).is_ok() && WinHttpReceiveResponse(r, std::ptr::null_mut()).is_ok() {
            let mut buf = [0u8; 4096];
            loop {
                let mut n = 0u32;
                if WinHttpReadData(r, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut n).is_err() || n == 0 { break; }
                text.extend_from_slice(&buf[..n as usize]);
            }
        }
        for h in [r, c, s] { if !h.is_null() { let _ = WinHttpCloseHandle(h); } }
        let t = String::from_utf8_lossy(&text);
        t.lines().find_map(|l| l.strip_prefix("ip=").map(String::from))
    }
}

// ---------- battery device ----------

fn battery_device() -> Option<Vec<u16>> {
    const GUID_BATTERY: GUID = GUID::from_u128(0x72631e54_78a4_11d0_bcf7_00aa00b7b32a);
    unsafe {
        let info = SetupDiGetClassDevsW(Some(&GUID_BATTERY), PCWSTR::null(), None, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE).ok()?;
        let mut d = SP_DEVICE_INTERFACE_DATA { cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32, ..Default::default() };
        let mut out = None;
        if SetupDiEnumDeviceInterfaces(info, None, &GUID_BATTERY, 0, &mut d).is_ok() {
            let mut need = 0u32;
            let _ = SetupDiGetDeviceInterfaceDetailW(info, &d, None, 0, Some(&mut need), None);
            if need > 0 {
                let mut buf = vec![0u32; need as usize / 4 + 1];
                let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
                (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
                if SetupDiGetDeviceInterfaceDetailW(info, &d, Some(detail), need, None, None).is_ok() {
                    let p = PCWSTR((*detail).DevicePath.as_ptr());
                    out = Some(p.as_wide().iter().copied().chain([0]).collect());
                }
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(info);
        out
    }
}

const IOCTL_BATTERY_QUERY_TAG: u32 = 0x294040;
const IOCTL_BATTERY_QUERY_INFORMATION: u32 = 0x294044;
const IOCTL_BATTERY_QUERY_STATUS: u32 = 0x29404C;

#[repr(C)] #[derive(Default)] struct QueryInfo { tag: u32, level: u32, scale: u32 }
#[repr(C)] #[derive(Default)] struct BatInfo { caps: u32, tech: u8, res: [u8; 3], chem: [u8; 4], design: u32, full: u32, def1: u32, def2: u32, crit: u32, cycles: u32 }
#[repr(C)] #[derive(Default)] struct WaitStatus { tag: u32, timeout: u32, state: u32, low: u32, high: u32 }
#[repr(C)] #[derive(Default)] struct Status { state: u32, cap: u32, voltage: u32, rate: i32 }

fn read_battery(path: &[u16], b: &mut Battery, stat: &mut Option<(Option<u32>, Option<u32>, Option<u32>)>) {
    unsafe {
        let Ok(h) = CreateFileW(PCWSTR(path.as_ptr()), (GENERIC_READ | GENERIC_WRITE).0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, None) else { return };
        let (mut tag, mut wait, mut n) = (0u32, 0u32, 0u32);
        if DeviceIoControl(h, IOCTL_BATTERY_QUERY_TAG, Some(&mut wait as *mut _ as *mut _), 4, Some(&mut tag as *mut _ as *mut _), 4, Some(&mut n), None).is_ok() && tag != 0 {
            if stat.is_none() {
                let q = QueryInfo { tag, level: 0, scale: 0 };
                let mut info = BatInfo::default();
                if DeviceIoControl(h, IOCTL_BATTERY_QUERY_INFORMATION, Some(&q as *const _ as *const _), 12, Some(&mut info as *mut _ as *mut _), std::mem::size_of::<BatInfo>() as u32, Some(&mut n), None).is_ok() {
                    let some = |v: u32| (v != 0 && v != u32::MAX).then_some(v);
                    *stat = Some((some(info.design), some(info.full), some(info.cycles)));
                }
            }
            let ws = WaitStatus { tag, ..Default::default() };
            let mut s = Status::default();
            if DeviceIoControl(h, IOCTL_BATTERY_QUERY_STATUS, Some(&ws as *const _ as *const _), std::mem::size_of::<WaitStatus>() as u32, Some(&mut s as *mut _ as *mut _), std::mem::size_of::<Status>() as u32, Some(&mut n), None).is_ok() {
                if s.voltage != u32::MAX { b.voltage_mv = Some(s.voltage); }
                if s.rate != i32::MIN { b.rate_mw = Some(s.rate); }
            }
        }
        if let Some((d, f, c)) = stat { b.design = *d; b.full = *f; b.cycles = *c; }
        let _ = CloseHandle(h);
    }
}
