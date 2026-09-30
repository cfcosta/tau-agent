package dev.cfcosta.tau;

import android.app.Activity;
import android.content.Intent;
import android.os.Build;

import java.io.File;
import java.util.concurrent.CountDownLatch;

/** What tau's Rust side asks of Android: a photo of a pairing code, and
 *  the phone's name. Called from native threads, never the UI's. */
public final class TauScanner {
    private TauScanner() {}

    /** Takes a photo with the camera app and returns its path, or null
     *  if the person backed out. Blocks until the camera closes. */
    public static String capture(Activity activity) throws InterruptedException {
        File dir = new File(activity.getCacheDir(), TauPhotos.DIR);
        dir.mkdirs();
        File photo = new File(dir, "code.jpg");
        photo.delete();
        CountDownLatch done = new CountDownLatch(1);
        TauCaptureActivity.pending = new TauCaptureActivity.Pending(photo, done);
        Intent intent = new Intent(activity, TauCaptureActivity.class);
        activity.startActivity(intent);
        done.await();
        String error = TauCaptureActivity.pending.error;
        TauCaptureActivity.pending = null;
        if (error != null) throw new IllegalStateException(error);
        return photo.length() > 0 ? photo.getAbsolutePath() : null;
    }

    /** The phone's model, as the computer lists it: "Pixel 9". */
    public static String deviceName() {
        return Build.MODEL;
    }
}
