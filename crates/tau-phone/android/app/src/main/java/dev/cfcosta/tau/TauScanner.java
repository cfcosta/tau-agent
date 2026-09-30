package dev.cfcosta.tau;

import android.app.Activity;
import android.content.Intent;
import android.os.Build;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.atomic.AtomicBoolean;

/** What tau's Rust side asks of Android: the pairing code, read in tau's
 *  viewfinder, and the phone's name. Called from native threads, never
 *  the UI's. */
public final class TauScanner {
    private TauScanner() {}

    /** One opening of the viewfinder, and how it ended. */
    static final class Scan {
        private final CountDownLatch done = new CountDownLatch(1);
        private final AtomicBoolean ended = new AtomicBoolean();
        private volatile String code;
        private volatile String error;

        /** The first ending counts: a code, an error, or neither for a
         *  person who backed out. Returns whether this one counted. */
        boolean end(String code, String error) {
            if (!ended.compareAndSet(false, true)) return false;
            this.code = code;
            this.error = error;
            done.countDown();
            return true;
        }

        boolean ended() {
            return ended.get();
        }
    }

    /** The viewfinder's scan while one is open. */
    static volatile Scan current;

    /** Opens the viewfinder and returns the pairing code it reads, or
     *  null if the person backed out. Blocks until the viewfinder closes;
     *  throws with what went wrong, in words. */
    public static String scan(Activity activity) throws InterruptedException {
        Scan scan = new Scan();
        Scan earlier = current;
        if (earlier != null) earlier.end(null, null);
        current = scan;
        activity.startActivity(new Intent(activity, TauViewfinder.class));
        try {
            scan.done.await();
        } finally {
            if (current == scan) current = null;
        }
        if (scan.error != null) throw new IllegalStateException(scan.error);
        return scan.code;
    }

    /** The phone's model, as the computer lists it: "Pixel 9". */
    public static String deviceName() {
        return Build.MODEL;
    }
}
