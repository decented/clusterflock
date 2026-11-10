//! Android Bluetooth LE: the native half of `BtBridge.java`.
//!
//! Java owns the radio and every blocking call (accept, connect, read, write,
//! and every binder call into the adapter, scanner and advertiser, which run
//! on BtBridge's worker thread). Rust calls BtBridge's static methods, which
//! only post, enqueue or read a volatile, and Java calls back through four native methods that this module registers
//! with `RegisterNatives` (through the app's class loader: NativeActivity
//! dlopens the .so itself, so neither `FindClass` from a native thread nor the
//! JVM's `Java_*` symbol lookup would find them). The same functions are also
//! exported under their `Java_…` names.
//!
//! Threads: the native callbacks run on Java threads and never block (packets
//! go to tokio channels with `try_send`; a full channel drops the packet, QUIC
//! recovers). Each link has one Rust "send pump" thread that waits on the
//! link's outgoing channel and hands packets to `BtBridge.send`, which only
//! enqueues. Nothing here touches `AndroidApp` (its lock deadlocks the game
//! thread) or blocks in a JNI read.
//!
//! RFCOMM, a diagnostic only: `adb shell setprop debug.flock.bt rfcomm`
//! makes the host listen on insecure RFCOMM and advertise PSM 0; a joiner
//! that sees PSM 0 connects with RFCOMM. It most likely fails between
//! unpaired phones: the joiner only knows the host's random LE address, and
//! RFCOMM pages the classic one.

use super::android::with_env;
use super::ble::{Ble, BleAdvert, BleSeen, BleState, HostLinks, LinkChannels, SeenTable, HOST_QUEUE, LINK_QUEUE, PSM_RFCOMM};
use jni::objects::{GlobalRef, JByteArray, JClass, JObject, JString, JValue, JValueOwned};
use jni::sys::{jint, jlong};
use jni::{JNIEnv, JavaVM, NativeMethod};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const BRIDGE: &str = "org.decented.gameflock.BtBridge";
/// Native-chosen link ids (our connects) start here; Java numbers accepted
/// links from 1, so the two never collide.
const NATIVE_LINK_BASE: i64 = 1 << 40;
/// `state()` is two JNI calls: re-read at most this often.
const STATE_TTL: Duration = Duration::from_millis(500);
/// After a failed BtBridge lookup, don't retry (and log) every frame.
const BRIDGE_RETRY: Duration = Duration::from_secs(5);

// BtBridge constants.
const PERM_UNKNOWN: i32 = 0;
const PERM_GRANTED: i32 = 1;
const ADAPTER_UNSUPPORTED: i32 = 0;
const ADAPTER_ON: i32 = 2;
// BtBridge.advertiseStatus() / scanStatus() codes of its own.
const ADVERT_NO_ADVERTISER: i32 = -100;
const ADVERT_LISTEN_FAILED: i32 = -101;
const SCAN_LOCATION_OFF: i32 = -200;
const SCAN_NO_SCANNER: i32 = -201;

struct Registry {
    bridge: Option<GlobalRef>,
    /// The JVM, kept from the first bridge lookup: send pumps attach with it
    /// even after android-activity has released `ndk_context` (teardown).
    vm: Option<JavaVM>,
    bridge_failed: Option<Instant>,
    /// Packets from Java, by link id.
    links: HashMap<i64, mpsc::Sender<Vec<u8>>>,
    /// Where accepted links go while hosting.
    host: Option<mpsc::Sender<LinkChannels>>,
    seen: SeenTable,
    next_link: i64,
}

static REG: LazyLock<Mutex<Registry>> = LazyLock::new(|| {
    Mutex::new(Registry {
        bridge: None,
        vm: None,
        bridge_failed: None,
        links: HashMap::new(),
        host: None,
        seen: SeenTable::default(),
        next_link: NATIVE_LINK_BASE,
    })
});

fn reg() -> MutexGuard<'static, Registry> {
    REG.lock().unwrap_or_else(|e| e.into_inner())
}

/// The JVM saved by [`bridge`].
fn stored_vm() -> Option<JavaVM> {
    let ptr = reg().vm.as_ref()?.get_java_vm_pointer();
    // SAFETY: the process JavaVM, which lives as long as the process.
    unsafe { JavaVM::from_raw(ptr) }.ok()
}

fn vm() -> Option<JavaVM> {
    let ctx = std::panic::catch_unwind(ndk_context::android_context).ok()?;
    if ctx.vm().is_null() {
        return None;
    }
    // SAFETY: the pointer is the process JavaVM, set up by android-activity.
    unsafe { JavaVM::from_raw(ctx.vm().cast()) }.ok()
}

/// The natives BtBridge declares; names and signatures must match the Java
/// (`tools/build-apk.sh` checks them against the dex and the .so). The
/// descriptors must also match the `extern "system"` fns' arguments below
/// (not checked by the build): (J, JString), (J, JByteArray), (J),
/// (JString, JByteArray, jint).
fn natives() -> Vec<NativeMethod> {
    let m = |name: &str, sig: &str, f: *mut std::ffi::c_void| NativeMethod { name: name.into(), sig: sig.into(), fn_ptr: f };
    vec![
        m("onLinkOpened", "(JLjava/lang/String;)V", Java_org_decented_gameflock_BtBridge_onLinkOpened as *mut _),
        m("onLinkPacket", "(J[B)V", Java_org_decented_gameflock_BtBridge_onLinkPacket as *mut _),
        m("onLinkClosed", "(J)V", Java_org_decented_gameflock_BtBridge_onLinkClosed as *mut _),
        m("onScanResult", "(Ljava/lang/String;[BI)V", Java_org_decented_gameflock_BtBridge_onScanResult as *mut _),
    ]
}

/// BtBridge's class, loaded through the app's class loader with its natives
/// registered (once per process).
fn bridge() -> Option<GlobalRef> {
    {
        let r = reg();
        if let Some(b) = &r.bridge {
            return Some(b.clone());
        }
        if r.bridge_failed.is_some_and(|t| t.elapsed() < BRIDGE_RETRY) {
            return None;
        }
    }
    let found = with_env("bt: load BtBridge", |env| {
        let ctx = std::panic::catch_unwind(ndk_context::android_context).map_err(|_| jni::errors::Error::NullPtr("android context"))?;
        if ctx.context().is_null() {
            return Err(jni::errors::Error::NullPtr("android context"));
        }
        // SAFETY: the context android-activity registered with ndk_context
        // (with NativeActivity, the Activity's global reference). Only its
        // class loader is used, and the class is cached once found. Not
        // owned: never deleted.
        let ctx_obj = unsafe { JObject::from_raw(ctx.context().cast()) };
        let loader = env.call_method(&ctx_obj, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?.l()?;
        let name = env.new_string(BRIDGE)?;
        let cls = env.call_method(&loader, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", &[JValue::Object(&name)])?.l()?;
        let cls = JClass::from(cls);
        env.register_native_methods(&cls, &natives())?;
        Ok((env.new_global_ref(&cls)?, env.get_java_vm()?))
    });
    let mut r = reg();
    match found {
        Some((b, vm)) => {
            log::info!("bt: BtBridge ready, natives registered");
            r.bridge = Some(b.clone());
            r.vm = Some(vm);
            Some(b)
        }
        None => {
            log::warn!("bt: BtBridge unavailable");
            r.bridge_failed = Some(Instant::now());
            None
        }
    }
}

/// A primitive result of a BtBridge call.
#[derive(Clone, Copy)]
enum Prim {
    Int(i32),
    Void,
}

/// `BtBridge.<name><sig>(args)` on this thread; `None` on any failure (logged).
fn call(name: &str, sig: &str, args: &[JValue]) -> Option<Prim> {
    let b = bridge()?;
    with_env(name, |env| {
        let cls: &JClass = b.as_obj().into();
        Ok(match env.call_static_method(cls, name, sig, args)? {
            JValueOwned::Int(i) => Prim::Int(i),
            _ => Prim::Void,
        })
    })
}

fn call_int(name: &str) -> Option<i32> {
    match call(name, "()I", &[])? {
        Prim::Int(i) => Some(i),
        _ => None,
    }
}

fn call_void(name: &str) {
    let _ = call(name, "()V", &[]);
}

/// `debug.flock.bt` = "rfcomm": host on insecure RFCOMM instead of L2CAP.
fn use_rfcomm() -> bool {
    use std::ffi::{c_char, c_int, CStr};
    extern "C" {
        fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
    }
    // PROP_VALUE_MAX is 92 bytes including the terminator.
    let mut buf = [0 as c_char; 92];
    // SAFETY: the name is NUL-terminated and `buf` holds PROP_VALUE_MAX bytes,
    // the most bionic ever writes.
    let n = unsafe { __system_property_get(c"debug.flock.bt".as_ptr(), buf.as_mut_ptr()) };
    if n <= 0 {
        return false;
    }
    // SAFETY: bionic NUL-terminates the value within PROP_VALUE_MAX.
    unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().trim().eq_ignore_ascii_case("rfcomm")
}

fn advert_error_text(code: i32) -> String {
    match code {
        -1 => "The Bluetooth advert is too large for this phone.".into(),
        -2 => "Too many Bluetooth adverts are running on this phone.".into(),
        -3 => "Already advertising.".into(),
        -5 | ADVERT_NO_ADVERTISER => "This phone can't host Bluetooth games (no BLE advertising).".into(),
        ADVERT_LISTEN_FAILED => "Couldn't start a Bluetooth game. Try switching Bluetooth off and on.".into(),
        c => format!("Bluetooth advertising failed (code {}).", -c),
    }
}

fn scan_error_text(code: i32) -> String {
    match code {
        SCAN_LOCATION_OFF => "Switch Location on to find Bluetooth games (Android 10 and 11 need it).".into(),
        SCAN_NO_SCANNER => "Couldn't look for Bluetooth games. Try switching Bluetooth off and on.".into(),
        -6 => "Searching too often. Try again in 30 seconds.".into(),
        -2 | -3 => "Bluetooth search failed. Try switching Bluetooth off and on.".into(),
        -4 | -5 => "This phone can't search for Bluetooth games.".into(),
        c => format!("Bluetooth search failed (code {}).", -c),
    }
}

// ─── Send pump (one thread per link) ────────────────────────────────────────

/// Waits on the link's outgoing channel and enqueues each packet in Java.
/// Ends (closing the Java link) when every `tx` is dropped or Java has
/// closed the link.
fn spawn_pump(id: i64, bridge: GlobalRef, mut out: mpsc::Receiver<Vec<u8>>) {
    let spawned = std::thread::Builder::new().name(format!("bt-send-{id}")).spawn(move || {
        let vm = stored_vm().or_else(vm);
        let env = vm.as_ref().map(|vm| vm.attach_current_thread_permanently());
        let mut env = match env {
            Some(Ok(e)) => e,
            other => {
                log::warn!("bt: send pump {id}: no JNI env ({:?})", other.map(|r| r.err()));
                // Can't reach Java to hang up: at least stop routing to this link,
                // so the next packet from Java closes it (onLinkPacket).
                reg().links.remove(&id);
                return;
            }
        };
        let cls: &JClass = bridge.as_obj().into();
        while let Some(pkt) = out.blocking_recv() {
            let sent = env.with_local_frame(8, |env| -> jni::errors::Result<bool> {
                let arr = env.byte_array_from_slice(&pkt)?;
                env.call_static_method(cls, "send", "(J[B)Z", &[JValue::Long(id), JValue::Object(&arr)])?.z()
            });
            match sent {
                Ok(true) => {}
                Ok(false) => break, // Java closed the link
                Err(e) => {
                    clear_exception(&mut env);
                    log::warn!("bt: send on link {id} failed: {e}");
                    break;
                }
            }
        }
        close_link(&mut env, cls, id);
        reg().links.remove(&id);
        // The thread detaches from the JVM as it exits (attach_current_thread_permanently).
    });
    if let Err(e) = spawned {
        log::warn!("bt: couldn't start the send pump for link {id}: {e}");
    }
}

fn clear_exception(env: &mut JNIEnv) {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
}

/// `BtBridge.close(id)` with an env we already hold (idempotent in Java).
fn close_link(env: &mut JNIEnv, cls: &JClass, id: jlong) {
    if env.call_static_method(cls, "close", "(J)V", &[JValue::Long(id)]).is_err() {
        clear_exception(env);
    }
}

/// A fresh link's channels: (what the consumer gets, Java→Rust sender, Rust→Java receiver).
fn new_link(label: String) -> (LinkChannels, mpsc::Sender<Vec<u8>>, mpsc::Receiver<Vec<u8>>) {
    let (to_rust, rx) = mpsc::channel(LINK_QUEUE);
    let (tx, to_java) = mpsc::channel(LINK_QUEUE);
    (LinkChannels { peer_label: label, tx, rx }, to_rust, to_java)
}

// ─── The Ble implementation ─────────────────────────────────────────────────

pub struct AndroidBle {
    state: Mutex<Option<(Instant, BleState)>>,
}

impl Default for AndroidBle {
    fn default() -> Self {
        Self::new()
    }
}

impl AndroidBle {
    pub fn new() -> Self {
        AndroidBle { state: Mutex::new(None) }
    }

    fn forget_state(&self) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn read_state() -> BleState {
        let Some(adapter) = call_int("adapterState") else { return BleState::Unsupported };
        if adapter == ADAPTER_UNSUPPORTED {
            return BleState::Unsupported;
        }
        match call_int("permissionState").unwrap_or(PERM_UNKNOWN) {
            PERM_GRANTED => {}
            PERM_UNKNOWN => return BleState::NeedsPermission,
            _ => return BleState::PermissionDenied,
        }
        match adapter {
            ADAPTER_ON => BleState::Ready,
            // Off, or not read yet by BtBridge's worker (the first ~ms).
            _ => BleState::Off,
        }
    }

    fn ready(&self) -> Result<(), String> {
        self.forget_state();
        match self.state() {
            BleState::Ready => Ok(()),
            BleState::Off => Err("Bluetooth is off.".into()),
            BleState::NeedsPermission | BleState::PermissionDenied => Err("Cluster Flock needs the Bluetooth permission.".into()),
            BleState::Unsupported => Err(super::ble::UNSUPPORTED.into()),
        }
    }
}

impl Ble for AndroidBle {
    fn state(&self) -> BleState {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, st)) = *s {
            if at.elapsed() < STATE_TTL {
                return st;
            }
        }
        let old = s.map(|(_, st)| st);
        let st = Self::read_state();
        if old != Some(st) {
            log::info!("bt: state {st:?}");
        }
        *s = Some((Instant::now(), st));
        st
    }

    fn request_permission(&self) {
        call_void("requestPermissions");
        self.forget_state();
    }

    fn request_enable(&self) {
        call_void("requestEnable");
        self.forget_state();
    }

    fn open_settings(&self) {
        call_void("openSettings");
        self.forget_state();
    }

    fn start_advertising(&self, mut ad: BleAdvert) -> Result<HostLinks, String> {
        self.ready()?;
        let rfcomm = use_rfcomm();
        let b = bridge().ok_or_else(|| super::ble::UNSUPPORTED.to_string())?;
        let (tx, rx) = mpsc::channel(HOST_QUEUE);
        // Set before listening: an accept can only follow this.
        reg().host = Some(tx);
        // BtBridge's worker listens, writes the PSM into bytes 1..3 and
        // advertises; the outcome comes back as advertising_error().
        ad.psm = PSM_RFCOMM;
        let bytes = ad.encode();
        let posted = with_env("bt: startHost", |env| {
            let cls: &JClass = b.as_obj().into();
            let arr = env.byte_array_from_slice(&bytes)?;
            env.call_static_method(cls, "startHost", "(Z[B)V", &[JValue::Bool(rfcomm.into()), JValue::Object(&arr)])?;
            Ok(())
        });
        if posted.is_none() {
            reg().host = None;
            return Err(advert_error_text(ADVERT_LISTEN_FAILED));
        }
        log::info!("bt: hosting room {} on {}", ad.room_label(), if rfcomm { "RFCOMM" } else { "L2CAP" });
        Ok(rx)
    }

    fn stop_advertising(&self) {
        reg().host = None;
        call_void("stopHost");
    }

    fn advertising_error(&self) -> Option<String> {
        let c = call_int("advertiseStatus")?;
        (c < 0).then(|| advert_error_text(c))
    }

    fn start_scan(&self) -> Result<(), String> {
        self.ready()?;
        reg().seen.clear();
        if call("startScan", "()V", &[]).is_none() {
            return Err(scan_error_text(SCAN_NO_SCANNER));
        }
        // Location off (Android 10-11) is known at once; the rest arrives later.
        match call_int("scanStatus") {
            Some(SCAN_LOCATION_OFF) => Err(scan_error_text(SCAN_LOCATION_OFF)),
            _ => Ok(()),
        }
    }

    fn stop_scan(&self) {
        call_void("stopScan");
    }

    fn scan_error(&self) -> Option<String> {
        let c = call_int("scanStatus")?;
        (c < 0).then(|| scan_error_text(c))
    }

    fn seen(&self) -> Vec<BleSeen> {
        reg().seen.list(Instant::now())
    }

    fn connect(&self, device_id: &str) -> Result<LinkChannels, String> {
        self.ready()?;
        let bridge = bridge().ok_or_else(|| super::ble::UNSUPPORTED.to_string())?;
        let tail: String = device_id.chars().rev().take(5).collect::<Vec<_>>().into_iter().rev().collect();
        let (link, to_rust, to_java) = new_link(format!("BLE \u{2026}{tail}"));
        // Registered before Java can call back about this id.
        let (id, psm) = {
            let mut r = reg();
            let psm = r.seen.get(device_id).map(|s| s.advert.psm).ok_or("That Bluetooth game has gone.")?;
            let id = r.next_link;
            r.next_link += 1;
            r.links.insert(id, to_rust);
            (id, psm)
        };
        let rfcomm = psm == PSM_RFCOMM;
        let started = with_env("bt: connect", |env| {
            let cls: &JClass = bridge.as_obj().into();
            let dev = env.new_string(device_id)?;
            env.call_static_method(
                cls,
                "connect",
                "(JLjava/lang/String;IZ)Z",
                &[JValue::Long(id), JValue::Object(&dev), JValue::Int(psm as jint), JValue::Bool(rfcomm.into())],
            )?
            .z()
        })
        .unwrap_or(false);
        if !started {
            reg().links.remove(&id);
            return Err("Couldn't connect over Bluetooth.".into());
        }
        spawn_pump(id, bridge, to_java);
        Ok(link)
    }
}

// ─── Native methods (called by BtBridge on Java threads) ────────────────────

/// Runs a native callback body, never letting a panic unwind into the JVM.
fn guard(what: &str, f: impl FnOnce()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
        log::error!("bt: {what} panicked");
    }
}

/// `static native void onLinkOpened(long linkId, String label)`.
#[no_mangle]
pub extern "system" fn Java_org_decented_gameflock_BtBridge_onLinkOpened(mut env: JNIEnv, cls: JClass, id: jlong, label: JString) {
    guard("onLinkOpened", || {
        let label: String = match env.get_string(&label) {
            Ok(s) => s.into(),
            Err(_) => {
                clear_exception(&mut env);
                "BLE".into()
            }
        };
        let host = {
            let r = reg();
            if r.links.contains_key(&id) {
                log::info!("bt: link {id} open ({label})");
                return; // our own connect
            }
            if id >= NATIVE_LINK_BASE {
                // Our own connect, already closed (Rust dropped it or Java
                // reported the close first): never mistake it for a joiner.
                None
            } else {
                r.host.clone()
            }
        };
        // An accepted joiner.
        let Some(host) = host else {
            log::info!("bt: link {id} opened while not hosting (or already closed): closing");
            close_link(&mut env, &cls, id);
            return;
        };
        let bridge = match env.new_global_ref(&cls) {
            Ok(b) => b,
            Err(_) => {
                clear_exception(&mut env);
                close_link(&mut env, &cls, id);
                return;
            }
        };
        let (link, to_rust, to_java) = new_link(label.clone());
        reg().links.insert(id, to_rust);
        match host.try_send(link) {
            Ok(()) => {
                log::info!("bt: joiner link {id} open ({label})");
                spawn_pump(id, bridge, to_java);
            }
            Err(e) => {
                log::warn!("bt: joiner link {id} refused: {e}");
                reg().links.remove(&id);
                close_link(&mut env, &cls, id);
            }
        }
    });
}

/// `static native void onLinkPacket(long linkId, byte[] data)`.
#[no_mangle]
pub extern "system" fn Java_org_decented_gameflock_BtBridge_onLinkPacket(mut env: JNIEnv, cls: JClass, id: jlong, data: JByteArray) {
    guard("onLinkPacket", || {
        let bytes = match env.convert_byte_array(&data) {
            Ok(b) => b,
            Err(_) => {
                clear_exception(&mut env);
                return;
            }
        };
        let closed = {
            let mut r = reg();
            match r.links.get(&id).map(|s| s.try_send(bytes)) {
                // Full: dropped (QUIC recovers).
                Some(Ok(())) | Some(Err(mpsc::error::TrySendError::Full(_))) => false,
                // Nobody routes this link any more (it was registered before
                // Java could read from it, so it was removed): hang up.
                None => true,
                Some(Err(mpsc::error::TrySendError::Closed(_))) => {
                    r.links.remove(&id);
                    true
                }
            }
        };
        if closed {
            // The consumer dropped its rx: hang up.
            close_link(&mut env, &cls, id);
        }
    });
}

/// `static native void onLinkClosed(long linkId)`.
#[no_mangle]
pub extern "system" fn Java_org_decented_gameflock_BtBridge_onLinkClosed(_env: JNIEnv, _cls: JClass, id: jlong) {
    guard("onLinkClosed", || {
        if reg().links.remove(&id).is_some() {
            log::info!("bt: link {id} closed");
        }
    });
}

/// `static native void onScanResult(String deviceId, byte[] serviceData, int rssi)`.
#[no_mangle]
pub extern "system" fn Java_org_decented_gameflock_BtBridge_onScanResult(mut env: JNIEnv, _cls: JClass, device: JString, data: JByteArray, rssi: jint) {
    guard("onScanResult", || {
        let device_id: String = match env.get_string(&device) {
            Ok(s) => s.into(),
            Err(_) => {
                clear_exception(&mut env);
                return;
            }
        };
        let bytes = match env.convert_byte_array(&data) {
            Ok(b) => b,
            Err(_) => {
                clear_exception(&mut env);
                return;
            }
        };
        let Some(advert) = BleAdvert::decode(&bytes) else {
            log::debug!("bt: ignoring an advert of another version ({} bytes)", bytes.len());
            return;
        };
        // Aged out and capped on every insert (SEEN_TTL, SEEN_MAX).
        if reg().seen.insert(BleSeen { device_id, advert, rssi, last_seen: Instant::now() }) {
            log::info!("bt: found room {} (psm {}, rssi {rssi})", advert.room_label(), advert.psm);
        }
    });
}
