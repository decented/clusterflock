package org.decented.gameflock;

import java.io.EOFException;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;

/**
 * Packet framing on a Bluetooth byte stream (L2CAP CoC or RFCOMM): a u16
 * big-endian length, then that many bytes, 0..MAX. Pure java.io (no android.*)
 * so tools/build-apk.sh can unit-test it on the host JDK (FramingTest). The
 * Rust port in app/src/platform/ble.rs follows the same rules.
 */
final class Framing {
    /** Largest payload (ble.rs MAX_FRAME). */
    static final int MAX = 2048;

    private Framing() {}

    /** One frame: header and payload in a single write. */
    static void write(OutputStream out, byte[] data) throws IOException {
        if (data.length > MAX) {
            throw new IOException("frame too long: " + data.length);
        }
        byte[] f = new byte[2 + data.length];
        f[0] = (byte) (data.length >>> 8);
        f[1] = (byte) data.length;
        System.arraycopy(data, 0, f, 2, data.length);
        out.write(f);
        out.flush();
    }

    /**
     * The next frame, or null at a clean end of stream (between frames).
     * Throws on a length over MAX (a corrupt stream) or an end mid-frame.
     * Reassembles frames from reads of any size.
     */
    static byte[] read(InputStream in) throws IOException {
        int hi = in.read();
        if (hi < 0) {
            return null;
        }
        int lo = in.read();
        if (lo < 0) {
            throw new EOFException("end of stream inside a frame header");
        }
        int n = (hi << 8) | lo;
        if (n > MAX) {
            throw new IOException("frame too long: " + n);
        }
        byte[] data = new byte[n];
        int at = 0;
        while (at < n) {
            int r = in.read(data, at, n - at);
            if (r < 0) {
                throw new EOFException("end of stream inside a frame");
            }
            at += r;
        }
        return data;
    }
}
