package dev.cfcosta.tau;

import android.Manifest;
import android.content.Context;
import android.content.pm.PackageManager;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.RectF;
import android.graphics.drawable.GradientDrawable;
import android.os.Build;
import android.os.Bundle;
import android.text.Layout;
import android.text.StaticLayout;
import android.text.TextPaint;
import android.util.Log;
import android.util.Size;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.HapticFeedbackConstants;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowManager;
import android.widget.FrameLayout;
import android.widget.TextView;

import androidx.activity.ComponentActivity;
import androidx.activity.EdgeToEdge;
import androidx.activity.SystemBarStyle;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.camera.core.Camera;
import androidx.camera.core.CameraSelector;
import androidx.camera.core.ImageAnalysis;
import androidx.camera.core.ImageProxy;
import androidx.camera.core.Preview;
import androidx.camera.core.resolutionselector.ResolutionSelector;
import androidx.camera.core.resolutionselector.ResolutionStrategy;
import androidx.camera.lifecycle.ProcessCameraProvider;
import androidx.camera.view.PreviewView;
import androidx.core.content.ContextCompat;
import androidx.core.graphics.Insets;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;

import com.google.common.util.concurrent.ListenableFuture;

import java.nio.ByteBuffer;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/** tau's viewfinder: the camera's preview, full screen, read frame by
 *  frame until the computer's pairing code is in view. The frames'
 *  luminance goes to tau's Rust decoder; the ending goes back to
 *  TauScanner. */
public class TauViewfinder extends ComponentActivity {
    private static final String TAG = "tau";
    /** tau's accent. */
    private static final int ACCENT = 0xffe5a54a;
    private static final String DENIED =
            "tau needs the camera to read the pairing code. Allow it in "
            + "Settings › Apps › tau › Permissions, or enter the address "
            + "instead.";

    /** The pairing code in a frame's luminance, or null: tau's Rust
     *  decoder, in libtau_phone.so, which the main activity loaded. */
    private static native String decode(ByteBuffer luma, int width, int height,
            int stride);

    private final ActivityResultLauncher<String> permission =
            registerForActivityResult(new ActivityResultContracts.RequestPermission(),
                    granted -> {
                        if (granted) open();
                        else end(null, DENIED);
                    });

    private TauScanner.Scan scan;
    /** One thread: a frame is read while the camera drops the rest. */
    private ExecutorService reader;
    private PreviewView preview;
    private TextView torch;
    private ImageAnalysis analysis;
    private Camera camera;
    private boolean lit;
    /** A copy for the rare camera whose frames are not direct buffers. */
    private ByteBuffer copy;

    @Override protected void onCreate(Bundle state) {
        EdgeToEdge.enable(this, SystemBarStyle.dark(Color.TRANSPARENT),
                SystemBarStyle.dark(Color.TRANSPARENT));
        super.onCreate(state);
        scan = TauScanner.current;
        if (scan == null || scan.ended()) {
            finish();
            return;
        }
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        reader = Executors.newSingleThreadExecutor();
        setContentView(layout());
        if (!getPackageManager().hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY)) {
            end(null, "This phone has no camera. Enter the address instead.");
        } else if (checkSelfPermission(Manifest.permission.CAMERA)
                == PackageManager.PERMISSION_GRANTED) {
            open();
        } else if (state == null) {
            // Recreated, the request under way still answers.
            permission.launch(Manifest.permission.CAMERA);
        }
    }

    @Override protected void onResume() {
        super.onResume();
        if (scan != null && scan.ended()) finish();
    }

    @Override protected void onDestroy() {
        super.onDestroy();
        // Turned, it comes back; otherwise it closed without a code.
        if (scan != null && !isChangingConfigurations()) scan.end(null, null);
        if (analysis != null) analysis.clearAnalyzer();
        if (reader != null) reader.shutdown();
    }

    /** Binds the preview and the reader to this activity's lifecycle:
     *  CameraX stops the camera while it is stopped, and lets go of it
     *  when it is destroyed. */
    private void open() {
        ListenableFuture<ProcessCameraProvider> future =
                ProcessCameraProvider.getInstance(this);
        future.addListener(() -> {
            if (isFinishing() || isDestroyed()) return;
            try {
                bind(future.get());
            } catch (Exception error) {
                Log.e(TAG, "the camera did not open", error);
                Throwable cause = error.getCause() != null ? error.getCause() : error;
                end(null, "The camera did not open: " + cause.getMessage());
            }
        }, ContextCompat.getMainExecutor(this));
    }

    private void bind(ProcessCameraProvider provider) throws Exception {
        CameraSelector selector = provider.hasCamera(CameraSelector.DEFAULT_BACK_CAMERA)
                ? CameraSelector.DEFAULT_BACK_CAMERA
                : CameraSelector.DEFAULT_FRONT_CAMERA;
        Preview shown = new Preview.Builder().build();
        shown.setSurfaceProvider(preview.getSurfaceProvider());
        // Enough pixels for a code that fills the square; the decoder
        // takes longer on more.
        ResolutionSelector resolution = new ResolutionSelector.Builder()
                .setResolutionStrategy(new ResolutionStrategy(new Size(1280, 720),
                        ResolutionStrategy.FALLBACK_RULE_CLOSEST_HIGHER_THEN_LOWER))
                .build();
        analysis = new ImageAnalysis.Builder()
                .setResolutionSelector(resolution)
                .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                .build();
        analysis.setAnalyzer(reader, this::read);
        provider.unbindAll();
        camera = provider.bindToLifecycle(this, selector, shown, analysis);
        if (camera.getCameraInfo().hasFlashUnit()) torch.setVisibility(View.VISIBLE);
    }

    /** On the reader's thread, one frame at a time. */
    private void read(ImageProxy frame) {
        try {
            if (scan.ended() || isDestroyed()) return;
            ImageProxy.PlaneProxy y = frame.getPlanes()[0];
            ByteBuffer luma = y.getBuffer();
            if (!luma.isDirect()) {
                if (copy == null || copy.capacity() < luma.remaining()) {
                    copy = ByteBuffer.allocateDirect(luma.remaining());
                }
                copy.clear();
                copy.put(luma.duplicate());
                luma = copy;
            }
            String code = decode(luma, frame.getWidth(), frame.getHeight(),
                    y.getRowStride());
            if (code != null) runOnUiThread(() -> found(code));
        } catch (RuntimeException error) {
            Log.w(TAG, "a frame did not read", error);
        } finally {
            frame.close();
        }
    }

    private void found(String code) {
        if (isDestroyed() || !scan.end(code, null)) return;
        preview.performHapticFeedback(Build.VERSION.SDK_INT >= Build.VERSION_CODES.R
                ? HapticFeedbackConstants.CONFIRM
                : HapticFeedbackConstants.VIRTUAL_KEY);
        finish();
    }

    private void end(String code, String error) {
        scan.end(code, error);
        finish();
    }

    private void toggleTorch() {
        if (camera == null) return;
        lit = !lit;
        camera.getCameraControl().enableTorch(lit);
        torch.setSelected(lit);
        pill(torch, lit);
    }

    private View layout() {
        FrameLayout root = new FrameLayout(this);
        root.setBackgroundColor(Color.BLACK);

        preview = new PreviewView(this);
        preview.setScaleType(PreviewView.ScaleType.FILL_CENTER);
        root.addView(preview, match());
        root.addView(new Guide(this), match());

        FrameLayout bar = new FrameLayout(this);
        int pad = dp(8);
        bar.setPadding(pad, pad, pad, pad);

        TextView close = button("✕");
        close.setTextSize(TypedValue.COMPLEX_UNIT_SP, 22);
        close.setContentDescription("Close");
        close.setOnClickListener(view -> end(null, null));
        bar.addView(close, new FrameLayout.LayoutParams(dp(48), dp(48),
                Gravity.START | Gravity.TOP));

        torch = button("Light");
        torch.setPadding(dp(16), 0, dp(16), 0);
        torch.setContentDescription("Flashlight");
        torch.setVisibility(View.GONE);
        pill(torch, false);
        torch.setOnClickListener(view -> toggleTorch());
        bar.addView(torch, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, dp(48), Gravity.END | Gravity.TOP));

        root.addView(bar, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.TOP));
        // Below the status bar and clear of a cutout.
        ViewCompat.setOnApplyWindowInsetsListener(bar, (view, insets) -> {
            Insets bars = insets.getInsets(WindowInsetsCompat.Type.systemBars()
                    | WindowInsetsCompat.Type.displayCutout());
            view.setPadding(pad + bars.left, pad + bars.top, pad + bars.right, pad);
            return insets;
        });
        return root;
    }

    private TextView button(String text) {
        TextView button = new TextView(this);
        button.setText(text);
        button.setTextColor(Color.WHITE);
        button.setTextSize(TypedValue.COMPLEX_UNIT_SP, 15);
        button.setGravity(Gravity.CENTER);
        button.setClickable(true);
        button.setFocusable(true);
        GradientDrawable back = new GradientDrawable();
        back.setShape(GradientDrawable.OVAL);
        back.setColor(0x66000000);
        button.setBackground(back);
        return button;
    }

    private void pill(TextView button, boolean on) {
        GradientDrawable back = new GradientDrawable();
        back.setCornerRadius(dp(24));
        back.setColor(on ? ACCENT : 0x66000000);
        button.setBackground(back);
        button.setTextColor(on ? Color.BLACK : Color.WHITE);
    }

    private static FrameLayout.LayoutParams match() {
        return new FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT);
    }

    private int dp(float value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }

    /** The square to hold the code in, the dimmed frame around it, and
     *  the hint under it. */
    private static final class Guide extends View {
        private static final String HINT =
                "Point at the code on your computer's Phones screen";
        private final Paint scrim = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Paint corner = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final TextPaint text = new TextPaint(Paint.ANTI_ALIAS_FLAG);
        private final Path path = new Path();
        private final RectF square = new RectF();
        private final float density;

        Guide(Context context) {
            super(context);
            density = context.getResources().getDisplayMetrics().density;
            scrim.setColor(0x99000000);
            corner.setColor(ACCENT);
            corner.setStyle(Paint.Style.STROKE);
            corner.setStrokeWidth(4 * density);
            corner.setStrokeCap(Paint.Cap.ROUND);
            text.setColor(Color.WHITE);
            text.setTextSize(16 * density * getResources().getConfiguration().fontScale);
        }

        @Override protected void onDraw(Canvas canvas) {
            float w = getWidth(), h = getHeight();
            float side = Math.min(w, h) * 0.7f;
            float left = (w - side) / 2, top = (h - side) / 2;
            square.set(left, top, left + side, top + side);
            float round = 16 * density;

            path.reset();
            path.setFillType(Path.FillType.EVEN_ODD);
            path.addRect(0, 0, w, h, Path.Direction.CW);
            path.addRoundRect(square, round, round, Path.Direction.CW);
            canvas.drawPath(path, scrim);

            float arm = side * 0.14f;
            float r = square.right, b = square.bottom;
            canvas.drawLine(left, top + arm, left, top + round, corner);
            canvas.drawLine(left + round, top, left + arm, top, corner);
            canvas.drawArc(left, top, left + 2 * round, top + 2 * round, 180, 90, false, corner);
            canvas.drawLine(r - arm, top, r - round, top, corner);
            canvas.drawLine(r, top + round, r, top + arm, corner);
            canvas.drawArc(r - 2 * round, top, r, top + 2 * round, 270, 90, false, corner);
            canvas.drawLine(left, b - arm, left, b - round, corner);
            canvas.drawLine(left + round, b, left + arm, b, corner);
            canvas.drawArc(left, b - 2 * round, left + 2 * round, b, 90, 90, false, corner);
            canvas.drawLine(r - arm, b, r - round, b, corner);
            canvas.drawLine(r, b - round, r, b - arm, corner);
            canvas.drawArc(r - 2 * round, b - 2 * round, r, b, 0, 90, false, corner);

            int width = (int) Math.max(1, Math.min(w - 48 * density, side + 48 * density));
            StaticLayout hint = StaticLayout.Builder
                    .obtain(HINT, 0, HINT.length(), text, width)
                    .setAlignment(Layout.Alignment.ALIGN_CENTER)
                    .build();
            // Under the square; turned sideways, over it, or at its top
            // when there is no room either side.
            float gap = 24 * density;
            float below = b + gap;
            if (below + hint.getHeight() > h) below = top - gap - hint.getHeight();
            if (below < 0) below = top + gap;
            canvas.save();
            canvas.translate((w - width) / 2, below);
            hint.draw(canvas);
            canvas.restore();
        }
    }
}
