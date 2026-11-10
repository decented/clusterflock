package org.decented.gameflock;

import android.Manifest;
import android.app.Activity;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothDevice;
import android.bluetooth.BluetoothManager;
import android.bluetooth.BluetoothServerSocket;
import android.bluetooth.BluetoothSocket;
import android.bluetooth.le.AdvertiseCallback;
import android.bluetooth.le.AdvertiseData;
import android.bluetooth.le.AdvertiseSettings;
import android.bluetooth.le.BluetoothLeAdvertiser;
import android.bluetooth.le.BluetoothLeScanner;
import android.bluetooth.le.ScanCallback;
import android.bluetooth.le.ScanFilter;
import android.bluetooth.le.ScanRecord;
import android.bluetooth.le.ScanResult;
import android.bluetooth.le.ScanSettings;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.location.LocationManager;
import android.net.Uri;
import android.os.Build;
import android.os.ParcelUuid;
import android.os.SystemClock;
import android.provider.Settings;
import android.util.Log;
import java.io.BufferedInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.lang.ref.WeakReference;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.Executors;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Bluetooth LE for off-grid games (app/src/platform/ble_android.rs is the
 * native side). The host listens on an insecure L2CAP CoC and advertises one
 * legacy advert: flags + 128-bit service data (AD type 0x21) under the app's
 * UUID, 8 bytes = version, PSM, room id, colour (29 of 31 bytes; no name, no
 * scan response). Joiners scan with a filter on that service data's version
 * byte and connect to the PSM.
 *
 * Threads. Native never waits on Bluetooth: every call into the adapter,
 * scanner or advertiser (binder calls, some slow on a sick stack) runs on the
 * single "flock-bt" worker; native's calls only post to it and return, and
 * state comes back through volatile fields native polls (adapterState,
 * permissionState, advertiseStatus, scanStatus). Java owns an accept thread
 * per host, a connect thread per outgoing link, and a reader and a writer per
 * link. Frames (Framing: u16 BE length + bytes) go to native through the
 * registered natives below; send() only enqueues (a full queue drops: QUIC
 * recovers). Every entry point catches everything: Bluetooth trouble is
 * logged, never a crash.
 *
 * RFCOMM (debug.flock.bt=rfcomm on the host) is a diagnostic only: the joiner
 * knows only the host's LE (random) address, and RFCOMM pages the classic
 * address, so between unpaired phones it most likely fails.
 */
public final class BtBridge {
    private static final String TAG = "flock";
    static final UUID SERVICE = UUID.fromString("38fbf737-1b3e-44d9-a432-3f94fc568c57");
    private static final ParcelUuid SERVICE_P = new ParcelUuid(SERVICE);
    private static final String RFCOMM_NAME = "ClusterFlock";
    /** ble.rs ADVERT_VERSION / ADVERT_LEN. */
    private static final byte ADVERT_VERSION = 1;
    private static final int ADVERT_LEN = 8;

    /** permissionState() values. */
    static final int PERM_UNKNOWN = 0, PERM_GRANTED = 1, PERM_DENIED = 2, PERM_PERMANENTLY_DENIED = 3;
    /** adapterState() values; UNKNOWN until the worker has looked once. */
    static final int ADAPTER_UNKNOWN = -1, ADAPTER_UNSUPPORTED = 0, ADAPTER_OFF = 1, ADAPTER_ON = 2;
    /** advertiseStatus(): 0 idle or starting, 1 advertising, negative = -AdvertiseCallback code or: */
    static final int ADVERT_NO_ADVERTISER = -100, ADVERT_LISTEN_FAILED = -101;
    /** scanStatus(): 0 idle or starting, 1 scanning, negative = -ScanCallback code or: */
    static final int SCAN_LOCATION_OFF = -200, SCAN_NO_SCANNER = -201;

    static final int REQ_PERMISSIONS = 0x7b1e;
    static final int REQ_ENABLE = 0x7b1f;
    private static final int QUEUE = 256;
    /** Accepted links open at once; more are hung up at once. */
    private static final int MAX_ACCEPTED = 40;
    /** An accepted link that sends no frame this soon is hung up. */
    private static final long FIRST_FRAME_MS = 15_000;
    /** Scanned devices remembered (ble.rs SEEN_MAX). */
    private static final int MAX_DEVICES = 64;
    private static final long STATE_REFRESH_MS = 250;
    private static final byte[] POISON = new byte[0];

    /** The application context (system services): strong, outlives activities. */
    private static volatile Context sApp;
    /** The activity, for prompts only. */
    private static volatile WeakReference<Activity> sActivity = new WeakReference<>(null);

    private static volatile int sPermResult = PERM_UNKNOWN;
    private static volatile boolean sPermGranted;
    private static volatile int sAdapterState = ADAPTER_UNKNOWN;
    private static volatile boolean sLocationOff;
    private static volatile long sStateAt;
    private static final AtomicBoolean sRefreshQueued = new AtomicBoolean();
    private static volatile int sAdvertStatus = 0;
    private static volatile int sScanStatus = 0;

    /** The worker: every adapter/scanner/advertiser call, in order. */
    private static final ScheduledExecutorService BT = Executors.newSingleThreadScheduledExecutor(r -> {
        Thread t = new Thread(r, "flock-bt");
        t.setDaemon(true);
        return t;
    });

    // Worker-only state (callbacks read the volatile copies to ignore stale ones).
    private static volatile BluetoothServerSocket sServer;
    private static volatile AdvertiseCallback sAdvertising;
    private static volatile ScanCallback sScanning;

    /** Devices from scan results, by address (connect needs the scanned object: random LE addresses). */
    private static final Map<String, BluetoothDevice> sDevices = Collections.synchronizedMap(new LinkedHashMap<String, BluetoothDevice>(16, 0.75f, true) {
        private static final long serialVersionUID = 1L;

        @Override
        protected boolean removeEldestEntry(Map.Entry<String, BluetoothDevice> e) {
            return size() > MAX_DEVICES;
        }
    });
    private static final Map<Long, Link> sLinks = new ConcurrentHashMap<>();
    private static final AtomicInteger sAccepted = new AtomicInteger();
    /** Ids of accepted links. Native picks ids for its connects from 1 << 40 up. */
    private static final AtomicLong sNextLink = new AtomicLong(1);

    // ─── Native callbacks (registered by ble_android.rs) ─────────────────────
    static native void onLinkOpened(long linkId, String label);
    static native void onLinkPacket(long linkId, byte[] data);
    static native void onLinkClosed(long linkId);
    static native void onScanResult(String deviceId, byte[] serviceData, int rssi);

    private BtBridge() {}

    private static void post(String what, Runnable r) {
        try {
            BT.execute(() -> {
                try {
                    r.run();
                } catch (Throwable t) {
                    Log.w(TAG, "bt: " + what, t);
                }
            });
        } catch (Throwable t) {
            Log.w(TAG, "bt: couldn't queue " + what, t);
        }
    }

    // ─── Activity, permissions, radio ────────────────────────────────────────

    /** FlockActivity.onCreate. */
    static void attach(Activity a) {
        sApp = a.getApplicationContext();
        sActivity = new WeakReference<>(a);
        requestRefresh();
    }

    private static Activity activity() {
        return sActivity.get();
    }

    /** Worker and link threads only (binder). */
    private static BluetoothAdapter adapter() {
        Context c = sApp;
        if (c == null) {
            return null;
        }
        BluetoothManager m = (BluetoothManager) c.getSystemService(Context.BLUETOOTH_SERVICE);
        return m == null ? null : m.getAdapter();
    }

    /** The runtime permissions Bluetooth games need on this Android version. */
    static String[] permissions() {
        if (Build.VERSION.SDK_INT >= 31) {
            return new String[] {
                Manifest.permission.BLUETOOTH_SCAN, Manifest.permission.BLUETOOTH_ADVERTISE, Manifest.permission.BLUETOOTH_CONNECT
            };
        }
        // API 29-30: scanning needs location (and Location switched on).
        return new String[] {Manifest.permission.ACCESS_FINE_LOCATION};
    }

    private static void requestRefresh() {
        if (sRefreshQueued.compareAndSet(false, true)) {
            post("refresh state", () -> {
                sRefreshQueued.set(false);
                refreshNow();
            });
        }
    }

    /** Worker: re-read the adapter, the permissions and (API < 31) Location. */
    private static void refreshNow() {
        Context c = sApp;
        int adapter = ADAPTER_UNSUPPORTED;
        boolean granted = false;
        boolean locationOff = false;
        try {
            if (c != null) {
                granted = true;
                for (String p : permissions()) {
                    if (c.checkSelfPermission(p) != PackageManager.PERMISSION_GRANTED) {
                        granted = false;
                    }
                }
                if (Build.VERSION.SDK_INT >= 29 && c.getPackageManager().hasSystemFeature(PackageManager.FEATURE_BLUETOOTH_LE)) {
                    BluetoothAdapter a = adapter();
                    adapter = a == null ? ADAPTER_UNSUPPORTED : a.isEnabled() ? ADAPTER_ON : ADAPTER_OFF;
                }
                if (Build.VERSION.SDK_INT >= 29 && Build.VERSION.SDK_INT < 31) {
                    LocationManager lm = (LocationManager) c.getSystemService(Context.LOCATION_SERVICE);
                    locationOff = lm != null && !lm.isLocationEnabled();
                }
            }
        } catch (Throwable t) {
            Log.w(TAG, "bt: state refresh", t);
            adapter = ADAPTER_OFF;
        }
        sPermGranted = granted;
        sAdapterState = adapter;
        sLocationOff = locationOff;
        sStateAt = SystemClock.elapsedRealtime();
    }

    private static void refreshIfStale() {
        if (SystemClock.elapsedRealtime() - sStateAt > STATE_REFRESH_MS) {
            requestRefresh();
        }
    }

    /** PERM_*: granted as last read by the worker; otherwise the last answer to our prompt. */
    static int permissionState() {
        refreshIfStale();
        if (sPermGranted) {
            return PERM_GRANTED;
        }
        // Granted once, revoked in Settings since: ask again.
        int r = sPermResult;
        return r == PERM_GRANTED ? PERM_UNKNOWN : r;
    }

    /** ADAPTER_* as last read by the worker (unsupported below API 29 or without BLE). */
    static int adapterState() {
        refreshIfStale();
        return sAdapterState;
    }

    /** Shows the system permission prompt (on the UI thread). */
    static void requestPermissions() {
        final Activity a = activity();
        if (a == null) {
            return;
        }
        a.runOnUiThread(() -> {
            try {
                Log.i(TAG, "bt: requesting permissions");
                a.requestPermissions(permissions(), REQ_PERMISSIONS);
            } catch (Throwable t) {
                Log.w(TAG, "bt: requestPermissions", t);
            }
        });
    }

    /** FlockActivity.onRequestPermissionsResult. */
    static void onPermissionsResult(Activity a, int requestCode, String[] perms, int[] results) {
        if (requestCode != REQ_PERMISSIONS) {
            return;
        }
        try {
            if (results == null || results.length == 0) {
                Log.i(TAG, "bt: permission prompt dismissed");
                return;
            }
            boolean all = true;
            boolean permanent = false;
            for (int i = 0; i < results.length; i++) {
                if (results[i] != PackageManager.PERMISSION_GRANTED) {
                    all = false;
                    // Denied and no rationale to show: "don't ask again".
                    if (!a.shouldShowRequestPermissionRationale(perms[i])) {
                        permanent = true;
                    }
                }
            }
            sPermResult = all ? PERM_GRANTED : permanent ? PERM_PERMANENTLY_DENIED : PERM_DENIED;
            Log.i(TAG, "bt: permissions " + (all ? "granted" : permanent ? "permanently denied" : "denied"));
        } catch (Throwable t) {
            Log.w(TAG, "bt: onPermissionsResult", t);
        } finally {
            requestRefresh();
        }
    }

    /** Asks the player to switch Bluetooth on (system dialog). */
    static void requestEnable() {
        final Activity a = activity();
        if (a == null) {
            return;
        }
        a.runOnUiThread(() -> {
            try {
                Log.i(TAG, "bt: asking to switch Bluetooth on");
                a.startActivityForResult(new Intent(BluetoothAdapter.ACTION_REQUEST_ENABLE), REQ_ENABLE);
            } catch (Throwable t) {
                Log.w(TAG, "bt: requestEnable", t);
            }
        });
    }

    /** The app's page in system Settings (after "don't ask again"). */
    static void openSettings() {
        final Activity a = activity();
        if (a == null) {
            return;
        }
        a.runOnUiThread(() -> {
            try {
                Intent i = new Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", a.getPackageName(), null));
                i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                a.startActivity(i);
            } catch (Throwable t) {
                Log.w(TAG, "bt: openSettings", t);
            }
        });
    }

    // ─── Socket type: the ONE place L2CAP vs RFCOMM is chosen ────────────────

    private static BluetoothServerSocket openServer(BluetoothAdapter a, boolean rfcomm) throws IOException {
        return rfcomm ? a.listenUsingInsecureRfcommWithServiceRecord(RFCOMM_NAME, SERVICE) : a.listenUsingInsecureL2capChannel();
    }

    private static BluetoothSocket openClient(BluetoothDevice d, int psm, boolean rfcomm) throws IOException {
        return rfcomm ? d.createInsecureRfcommSocketToServiceRecord(SERVICE) : d.createInsecureL2capChannel(psm);
    }

    // ─── Host ────────────────────────────────────────────────────────────────

    /**
     * Listen for joiners and advertise `serviceData` (8 bytes; the PSM is
     * written in here), replacing any previous host. Returns at once; the
     * outcome is advertiseStatus().
     */
    static void startHost(final boolean rfcomm, byte[] serviceData) {
        if (serviceData == null || serviceData.length != ADVERT_LEN) {
            sAdvertStatus = ADVERT_LISTEN_FAILED;
            return;
        }
        final byte[] data = serviceData.clone();
        sAdvertStatus = 0;
        post("startHost", () -> startHostNow(rfcomm, data));
    }

    private static void startHostNow(boolean rfcomm, byte[] data) {
        stopHostNow();
        sAdvertStatus = 0;
        BluetoothAdapter a = adapter();
        if (a == null || !a.isEnabled() || Build.VERSION.SDK_INT < 29) {
            sAdvertStatus = ADVERT_LISTEN_FAILED;
            return;
        }
        final BluetoothServerSocket server;
        int psm;
        try {
            server = openServer(a, rfcomm);
            psm = rfcomm ? 0 : server.getPsm();
        } catch (Throwable t) {
            Log.w(TAG, "bt: startHost (" + (rfcomm ? "RFCOMM" : "L2CAP") + ") failed", t);
            sAdvertStatus = ADVERT_LISTEN_FAILED;
            return;
        }
        if (!rfcomm && (psm < 0x80 || psm > 0xFF)) {
            // Joiners only accept the LE dynamic range (ble.rs LE_PSM_DYNAMIC).
            Log.w(TAG, "bt: unexpected L2CAP psm " + psm + " (outside 0x80..0xFF): joiners will ignore this advert");
        }
        sServer = server;
        Thread t = new Thread(() -> acceptLoop(server), "flock-bt-accept");
        t.setDaemon(true);
        t.start();
        Log.i(TAG, "bt: listening on " + (rfcomm ? "RFCOMM" : "L2CAP psm " + psm));
        data[1] = (byte) (psm >>> 8);
        data[2] = (byte) psm;
        startAdvertisingNow(a, data);
    }

    private static void acceptLoop(BluetoothServerSocket server) {
        try {
            while (true) {
                BluetoothSocket s = server.accept();
                if (sAccepted.get() >= MAX_ACCEPTED) {
                    Log.w(TAG, "bt: " + MAX_ACCEPTED + " links open: hanging up on a new one");
                    closeQuietly(s);
                    continue;
                }
                final long id = sNextLink.getAndIncrement();
                final Link l = new Link(id, label(s.getRemoteDevice()), true);
                sAccepted.incrementAndGet();
                sLinks.put(id, l);
                Log.i(TAG, "bt: accepted link " + id + " from " + l.label);
                // A link that never speaks would hold a seat: hang up.
                BT.schedule(() -> {
                    if (!l.closed && !l.gotFrame) {
                        Log.w(TAG, "bt: link " + id + " sent nothing in " + FIRST_FRAME_MS + " ms: closing");
                        closeLink(id);
                    }
                }, FIRST_FRAME_MS, TimeUnit.MILLISECONDS);
                l.start(s);
            }
        } catch (Throwable t) {
            // stopHost closed the server socket, or the radio went away.
            Log.i(TAG, "bt: accept loop ended (" + t.getMessage() + ")");
        }
    }

    /** Worker: one legacy connectable advert, flags + 128-bit service data, no scan response. */
    private static void startAdvertisingNow(BluetoothAdapter a, byte[] serviceData) {
        BluetoothLeAdvertiser adv = a.getBluetoothLeAdvertiser();
        if (adv == null) {
            sAdvertStatus = ADVERT_NO_ADVERTISER;
            Log.w(TAG, "bt: no BLE advertiser (peripheral mode unsupported or radio off)");
            return;
        }
        AdvertiseSettings settings = new AdvertiseSettings.Builder()
                .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_LOW_LATENCY)
                .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_HIGH)
                .setConnectable(true)
                .setTimeout(0)
                .build();
        // 3 (flags) + 2 + 16 + 8 = 29 of 31 bytes. Never a name.
        AdvertiseData data = new AdvertiseData.Builder()
                .addServiceData(SERVICE_P, serviceData)
                .setIncludeDeviceName(false)
                .setIncludeTxPowerLevel(false)
                .build();
        AdvertiseCallback cb = new AdvertiseCallback() {
            @Override
            public void onStartSuccess(AdvertiseSettings s) {
                if (sAdvertising == this) {
                    sAdvertStatus = 1;
                }
                Log.i(TAG, "bt: advertising");
            }

            @Override
            public void onStartFailure(int errorCode) {
                if (sAdvertising == this) {
                    sAdvertStatus = -errorCode;
                }
                Log.w(TAG, "bt: advertising failed, ADVERTISE_FAILED code " + errorCode);
            }
        };
        sAdvertising = cb;
        try {
            adv.startAdvertising(settings, data, cb);
        } catch (Throwable t) {
            sAdvertising = null;
            sAdvertStatus = -4;
            Log.w(TAG, "bt: startAdvertising", t);
        }
    }

    static int advertiseStatus() {
        return sAdvertStatus;
    }

    /** Stops advertising and accepting (returns at once). Links already open stay open. */
    static void stopHost() {
        post("stopHost", BtBridge::stopHostNow);
    }

    private static void stopHostNow() {
        AdvertiseCallback cb = sAdvertising;
        sAdvertising = null;
        if (cb != null) {
            try {
                BluetoothAdapter a = adapter();
                BluetoothLeAdvertiser adv = a == null ? null : a.getBluetoothLeAdvertiser();
                if (adv != null) {
                    adv.stopAdvertising(cb);
                }
            } catch (Throwable t) {
                Log.w(TAG, "bt: stopAdvertising", t);
            }
        }
        sAdvertStatus = 0;
        BluetoothServerSocket s = sServer;
        sServer = null;
        if (s != null) {
            try {
                s.close();
            } catch (Throwable t) {
                Log.w(TAG, "bt: closing the server socket", t);
            }
            Log.i(TAG, "bt: stopped hosting");
        }
    }

    // ─── Scan ────────────────────────────────────────────────────────────────

    /** Starts scanning (returns at once; the outcome is scanStatus()). */
    static void startScan() {
        sDevices.clear();
        // API 29-30 scans find nothing with Location off: say so at once.
        sScanStatus = Build.VERSION.SDK_INT < 31 && sLocationOff ? SCAN_LOCATION_OFF : 0;
        post("startScan", BtBridge::startScanNow);
    }

    private static void startScanNow() {
        stopScanNow();
        BluetoothAdapter a = adapter();
        BluetoothLeScanner sc = a == null ? null : a.getBluetoothLeScanner();
        if (sc == null) {
            sScanStatus = SCAN_NO_SCANNER;
            return;
        }
        if (Build.VERSION.SDK_INT < 31) {
            refreshNow();
            if (sLocationOff) {
                sScanStatus = SCAN_LOCATION_OFF;
                Log.w(TAG, "bt: Location is off: Android 10-11 scans find nothing");
            }
        }
        // Matches our service data with version byte ADVERT_VERSION.
        ScanFilter f = new ScanFilter.Builder()
                .setServiceData(SERVICE_P, new byte[] {ADVERT_VERSION}, new byte[] {(byte) 0xFF})
                .build();
        ScanSettings s = new ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY).build();
        ScanCallback cb = new ScanCallback() {
            @Override
            public void onScanResult(int callbackType, ScanResult r) {
                report(r);
            }

            @Override
            public void onBatchScanResults(List<ScanResult> rs) {
                for (ScanResult r : rs) {
                    report(r);
                }
            }

            @Override
            public void onScanFailed(int errorCode) {
                if (sScanning == this) {
                    sScanStatus = -errorCode;
                }
                Log.w(TAG, "bt: scan failed, SCAN_FAILED code " + errorCode);
            }
        };
        sScanning = cb;
        try {
            sc.startScan(Collections.singletonList(f), s, cb);
            if (sScanStatus == 0) {
                sScanStatus = 1;
            }
            Log.i(TAG, "bt: scanning");
        } catch (Throwable t) {
            sScanning = null;
            sScanStatus = -3;
            Log.w(TAG, "bt: startScan", t);
        }
    }

    static int scanStatus() {
        return sScanStatus;
    }

    private static void report(ScanResult r) {
        try {
            ScanRecord rec = r.getScanRecord();
            byte[] sd = rec == null ? null : rec.getServiceData(SERVICE_P);
            if (sd == null || sd.length < ADVERT_LEN || sd[0] != ADVERT_VERSION) {
                return; // native re-checks every field (BleAdvert::decode)
            }
            BluetoothDevice d = r.getDevice();
            String id = d.getAddress();
            sDevices.put(id, d);
            onScanResult(id, sd, r.getRssi());
        } catch (Throwable t) {
            Log.w(TAG, "bt: scan result", t);
        }
    }

    /** Stops scanning (returns at once). */
    static void stopScan() {
        post("stopScan", BtBridge::stopScanNow);
    }

    private static void stopScanNow() {
        ScanCallback cb = sScanning;
        sScanning = null;
        if (sScanStatus > 0) {
            sScanStatus = 0;
        }
        if (cb == null) {
            return;
        }
        try {
            BluetoothAdapter a = adapter();
            BluetoothLeScanner sc = a == null ? null : a.getBluetoothLeScanner();
            if (sc != null) {
                sc.stopScan(cb);
            }
            Log.i(TAG, "bt: scan stopped");
        } catch (Throwable t) {
            Log.w(TAG, "bt: stopScan", t);
        }
    }

    // ─── Join ────────────────────────────────────────────────────────────────

    /**
     * Connects in the background under native's linkId: onLinkOpened on
     * success, onLinkClosed on failure. Stops the scan first (it slows
     * connection setup). False only if the connect thread can't start.
     */
    static boolean connect(final long linkId, final String deviceId, final int psm, final boolean rfcomm) {
        try {
            if (Build.VERSION.SDK_INT < 29) {
                return false;
            }
            final Link l = new Link(linkId, "BLE", false);
            sLinks.put(linkId, l);
            stopScan();
            Thread t = new Thread(() -> {
                try {
                    BluetoothDevice dev = sDevices.get(deviceId);
                    if (dev == null) {
                        BluetoothAdapter a = adapter();
                        if (a == null) {
                            throw new IOException("no Bluetooth adapter");
                        }
                        dev = a.getRemoteDevice(deviceId);
                    }
                    l.label = label(dev);
                    Log.i(TAG, "bt: connecting link " + linkId + " to " + l.label + (rfcomm ? " (RFCOMM)" : " (L2CAP psm " + psm + ")"));
                    BluetoothSocket s = openClient(dev, psm, rfcomm);
                    l.socket = s; // so close() can abort the connect
                    if (l.closed) {
                        closeQuietly(s);
                        return;
                    }
                    s.connect();
                    l.start(s);
                } catch (Throwable e) {
                    Log.w(TAG, "bt: connect link " + linkId + " failed: " + e);
                    closeLink(linkId);
                }
            }, "flock-bt-connect");
            t.setDaemon(true);
            t.start();
            return true;
        } catch (Throwable t) {
            Log.w(TAG, "bt: connect", t);
            sLinks.remove(linkId);
            return false;
        }
    }

    // ─── Links ───────────────────────────────────────────────────────────────

    /**
     * Queues one packet for the link's writer. Never blocks: a full queue or
     * an oversized packet is dropped. False only if the link is gone.
     */
    static boolean send(long linkId, byte[] data) {
        Link l = sLinks.get(linkId);
        if (l == null || l.closed) {
            return false;
        }
        if (data != null && data.length <= Framing.MAX) {
            l.out.offer(data);
        }
        return true;
    }

    /** Closes the link (idempotent); native hears onLinkClosed once. */
    static void close(long linkId) {
        closeLink(linkId);
    }

    private static void closeLink(long linkId) {
        Link l = sLinks.remove(linkId);
        if (l == null) {
            return;
        }
        l.closed = true;
        if (l.accepted) {
            sAccepted.decrementAndGet();
        }
        l.out.clear();
        l.out.offer(POISON);
        BluetoothSocket s = l.socket;
        if (s != null) {
            closeQuietly(s);
        }
        Log.i(TAG, "bt: link " + linkId + " closed");
        try {
            onLinkClosed(linkId);
        } catch (Throwable t) {
            Log.w(TAG, "bt: onLinkClosed", t);
        }
    }

    private static void closeQuietly(BluetoothSocket s) {
        try {
            s.close();
        } catch (Throwable ignored) {
            // closing anyway
        }
    }

    private static String label(BluetoothDevice d) {
        String a = d == null ? null : d.getAddress();
        if (a == null || a.length() < 5) {
            return "BLE";
        }
        return "BLE …" + a.substring(a.length() - 5);
    }

    private static final class Link {
        final long id;
        final boolean accepted;
        volatile String label;
        final LinkedBlockingQueue<byte[]> out = new LinkedBlockingQueue<>(QUEUE);
        volatile BluetoothSocket socket;
        volatile boolean closed;
        volatile boolean gotFrame;

        Link(long id, String label, boolean accepted) {
            this.id = id;
            this.label = label;
            this.accepted = accepted;
        }

        /**
         * Connected: tell native, then start the reader and writer. A link
         * closed meanwhile (by native or the watchdog) only closes its socket.
         */
        void start(BluetoothSocket s) {
            socket = s;
            if (closed) {
                closeQuietly(s);
                return;
            }
            try {
                onLinkOpened(id, label);
            } catch (Throwable t) {
                Log.w(TAG, "bt: onLinkOpened", t);
                closeLink(id);
                return;
            }
            if (closed) {
                closeQuietly(s);
                return;
            }
            Thread r = new Thread(this::readLoop, "flock-bt-read-" + id);
            Thread w = new Thread(this::writeLoop, "flock-bt-write-" + id);
            r.setDaemon(true);
            w.setDaemon(true);
            r.start();
            w.start();
        }

        private void readLoop() {
            try {
                InputStream in = new BufferedInputStream(socket.getInputStream(), 4096);
                while (!closed) {
                    byte[] f = Framing.read(in);
                    if (f == null) {
                        break;
                    }
                    gotFrame = true;
                    onLinkPacket(id, f);
                }
            } catch (Throwable t) {
                if (!closed) {
                    Log.i(TAG, "bt: link " + id + " read ended: " + t);
                }
            } finally {
                closeLink(id);
            }
        }

        private void writeLoop() {
            try {
                OutputStream o = socket.getOutputStream();
                while (!closed) {
                    byte[] f = out.take();
                    if (f == POISON) {
                        break;
                    }
                    Framing.write(o, f);
                }
            } catch (Throwable t) {
                if (!closed) {
                    Log.i(TAG, "bt: link " + id + " write ended: " + t);
                }
            } finally {
                closeLink(id);
            }
        }
    }
}
