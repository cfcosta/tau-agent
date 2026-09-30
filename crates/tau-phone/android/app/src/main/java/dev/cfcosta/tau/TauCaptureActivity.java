package dev.cfcosta.tau;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.os.Bundle;
import android.provider.MediaStore;

import java.io.File;
import java.util.concurrent.CountDownLatch;

/** A see-through activity that opens the camera app for one photo and
 *  hands the result back to TauScanner. */
public class TauCaptureActivity extends Activity {
    private static final int CAPTURE = 1;

    static final class Pending {
        final File photo;
        final CountDownLatch done;
        volatile String error;

        Pending(File photo, CountDownLatch done) {
            this.photo = photo;
            this.done = done;
        }
    }

    static volatile Pending pending;

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        // Recreated while the camera was up: its result still comes.
        if (state != null) return;
        Pending now = pending;
        if (now == null) {
            finish();
            return;
        }
        Intent intent = new Intent(MediaStore.ACTION_IMAGE_CAPTURE);
        intent.putExtra(MediaStore.EXTRA_OUTPUT, TauPhotos.uri(now.photo));
        intent.addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
        try {
            startActivityForResult(intent, CAPTURE);
        } catch (ActivityNotFoundException error) {
            now.error = "this phone has no camera app";
            finishWith(now);
        }
    }

    @Override protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        Pending now = pending;
        if (now == null) {
            finish();
            return;
        }
        if (result != RESULT_OK) now.photo.delete();
        finishWith(now);
    }

    private void finishWith(Pending now) {
        now.done.countDown();
        finish();
    }
}
