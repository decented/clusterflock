package org.decented.gameflock;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.EOFException;
import java.io.IOException;
import java.io.InputStream;
import java.util.Arrays;

/**
 * Host-JDK test of Framing (run by tools/build-apk.sh before d8; not in the
 * APK). Exit status 0 = pass.
 */
public final class FramingTest {
    private static int failures = 0;

    private static void check(boolean ok, String what) {
        if (!ok) {
            failures++;
            System.err.println("FAIL: " + what);
        }
    }

    /** Returns at most `chunk` bytes per read: L2CAP may deliver any split. */
    private static final class Trickle extends InputStream {
        private final InputStream in;
        private final int chunk;

        Trickle(byte[] b, int chunk) {
            this.in = new ByteArrayInputStream(b);
            this.chunk = chunk;
        }

        @Override
        public int read() throws IOException {
            return in.read();
        }

        @Override
        public int read(byte[] b, int off, int len) throws IOException {
            return in.read(b, off, Math.min(len, chunk));
        }
    }

    private static byte[] pattern(int n) {
        byte[] b = new byte[n];
        for (int i = 0; i < n; i++) {
            b[i] = (byte) (i * 7);
        }
        return b;
    }

    public static void main(String[] args) throws IOException {
        int[] sizes = {0, 1, 255, 256, 1200, 1452, Framing.MAX};
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        for (int n : sizes) {
            Framing.write(out, pattern(n));
        }
        byte[] stream = out.toByteArray();

        // Header: u16 big-endian length.
        check(stream[0] == 0 && stream[1] == 0, "empty frame header");
        check(stream[2] == 0 && stream[3] == 1, "1-byte frame header");
        ByteArrayOutputStream one = new ByteArrayOutputStream();
        Framing.write(one, pattern(Framing.MAX));
        byte[] h = one.toByteArray();
        check((h[0] & 0xff) == 0x08 && (h[1] & 0xff) == 0x00 && h.length == 2 + Framing.MAX, "max frame header");

        // Round trip, whole and reassembled from partial reads.
        for (int chunk : new int[] {1, 2, 3, 1000, stream.length}) {
            InputStream in = new Trickle(stream, chunk);
            for (int n : sizes) {
                byte[] got = Framing.read(in);
                check(got != null && Arrays.equals(got, pattern(n)), "round trip size " + n + " chunk " + chunk);
            }
            check(Framing.read(in) == null, "clean EOF after the last frame, chunk " + chunk);
        }

        // Too long to send.
        try {
            Framing.write(new ByteArrayOutputStream(), new byte[Framing.MAX + 1]);
            check(false, "write accepted an oversized frame");
        } catch (IOException expected) {
            // ok
        }

        // A corrupt length closes the link.
        try {
            Framing.read(new ByteArrayInputStream(new byte[] {0x08, 0x01, 0, 0}));
            check(false, "read accepted a length over MAX");
        } catch (EOFException e) {
            check(false, "oversized length reported as EOF");
        } catch (IOException expected) {
            // ok
        }

        // Truncated: end of stream inside the header or the payload.
        for (byte[] cut : new byte[][] {{0x00}, {0x00, 0x05, 1, 2}}) {
            try {
                Framing.read(new Trickle(cut, 1));
                check(false, "truncated frame accepted (" + cut.length + " bytes)");
            } catch (EOFException expected) {
                // ok
            }
        }

        if (failures > 0) {
            System.err.println("FramingTest: " + failures + " failure(s)");
            System.exit(1);
        }
        System.out.println("FramingTest: ok");
    }
}
