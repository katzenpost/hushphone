// SPDX-License-Identifier: AGPL-3.0-only
package network.katzenpost.hushphone;

import android.Manifest;
import android.app.Activity;
import android.content.pm.PackageManager;
import android.graphics.ImageFormat;
import android.graphics.Matrix;
import android.graphics.SurfaceTexture;
import android.hardware.camera2.CameraCaptureSession;
import android.hardware.camera2.CameraCharacteristics;
import android.hardware.camera2.CameraDevice;
import android.hardware.camera2.CameraManager;
import android.hardware.camera2.CaptureRequest;
import android.media.Image;
import android.media.ImageReader;
import android.os.Bundle;
import android.os.Handler;
import android.os.HandlerThread;
import android.util.Size;
import android.view.Surface;
import android.view.TextureView;
import android.view.WindowManager;
import java.util.Arrays;

/**
 * Shows what the camera sees and hands the text of the first QR code in it to the native code,
 * like FilePicker: nativeOnResult gets the text, or null if the user went back. The camera
 * permission is asked for here, when the user first scans. Frames go to nativeDecode, which
 * reads them with the app's QR decoder; nothing is kept.
 */
public final class QrScanner extends Activity implements TextureView.SurfaceTextureListener {
    static {
        System.loadLibrary("hushphone");
    }

    private TextureView view;
    private HandlerThread thread;
    private Handler handler;
    private CameraDevice camera;
    private ImageReader reader;
    private Size size;
    private boolean answered;

    @Override
    protected void onCreate(Bundle saved) {
        super.onCreate(saved);
        if (saved != null) {
            answer(null);
            return;
        }
        if (!"0".equals(getIntent().getStringExtra("secure"))) {
            getWindow().addFlags(WindowManager.LayoutParams.FLAG_SECURE);
        }
        view = new TextureView(this);
        setContentView(view);
        if (checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            start();
        } else {
            requestPermissions(new String[] {Manifest.permission.CAMERA}, 1);
        }
    }

    @Override
    public void onRequestPermissionsResult(int request, String[] permissions, int[] results) {
        if (results.length > 0 && results[0] == PackageManager.PERMISSION_GRANTED) {
            start();
        } else {
            answer(null);
        }
    }

    private void start() {
        if (view.isAvailable()) {
            open();
        } else {
            view.setSurfaceTextureListener(this);
        }
    }

    private void open() {
        thread = new HandlerThread("qr-scanner");
        thread.start();
        handler = new Handler(thread.getLooper());
        try {
            CameraManager cameras = getSystemService(CameraManager.class);
            String id = null;
            for (String c : cameras.getCameraIdList()) {
                Integer facing = cameras.getCameraCharacteristics(c).get(CameraCharacteristics.LENS_FACING);
                if (id == null || (facing != null && facing == CameraCharacteristics.LENS_FACING_BACK)) {
                    id = c;
                }
            }
            if (id == null) {
                answer(null);
                return;
            }
            // The largest frame up to 1080p: enough for a code across the screen, quick to read.
            size = null;
            for (Size s : cameras.getCameraCharacteristics(id).get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
                    .getOutputSizes(ImageFormat.YUV_420_888)) {
                if ((long) s.getWidth() * s.getHeight() <= 1920 * 1080
                        && (size == null || s.getWidth() * s.getHeight() > size.getWidth() * size.getHeight())) {
                    size = s;
                }
            }
            reader = ImageReader.newInstance(size.getWidth(), size.getHeight(), ImageFormat.YUV_420_888, 2);
            reader.setOnImageAvailableListener(new ImageReader.OnImageAvailableListener() {
                @Override
                public void onImageAvailable(ImageReader r) {
                    frame(r);
                }
            }, handler);
            cameras.openCamera(id, new CameraDevice.StateCallback() {
                @Override
                public void onOpened(CameraDevice c) {
                    camera = c;
                    session();
                }

                @Override
                public void onDisconnected(CameraDevice c) {
                    c.close();
                    later(null);
                }

                @Override
                public void onError(CameraDevice c, int error) {
                    c.close();
                    later(null);
                }
            }, handler);
        } catch (Exception e) {
            answer(null);
        }
    }

    @SuppressWarnings("deprecation")
    private void session() {
        try {
            SurfaceTexture texture = view.getSurfaceTexture();
            texture.setDefaultBufferSize(size.getWidth(), size.getHeight());
            runOnUiThread(new Runnable() {
                @Override
                public void run() {
                    fit();
                }
            });
            Surface preview = new Surface(texture);
            final CaptureRequest.Builder b = camera.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW);
            b.addTarget(preview);
            b.addTarget(reader.getSurface());
            b.set(CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_PICTURE);
            camera.createCaptureSession(Arrays.asList(preview, reader.getSurface()), new CameraCaptureSession.StateCallback() {
                @Override
                public void onConfigured(CameraCaptureSession s) {
                    try {
                        s.setRepeatingRequest(b.build(), null, handler);
                    } catch (Exception e) {
                        later(null);
                    }
                }

                @Override
                public void onConfigureFailed(CameraCaptureSession s) {
                    later(null);
                }
            }, handler);
        } catch (Exception e) {
            later(null);
        }
    }

    /** Fill the view with the preview, cropped rather than stretched. */
    private void fit() {
        float vw = view.getWidth(), vh = view.getHeight();
        boolean portrait = vh > vw;
        float bw = portrait ? size.getHeight() : size.getWidth(), bh = portrait ? size.getWidth() : size.getHeight();
        float s = Math.max(vw / bw, vh / bh);
        Matrix m = new Matrix();
        m.setScale(bw * s / vw, bh * s / vh, vw / 2, vh / 2);
        view.setTransform(m);
    }

    private void frame(ImageReader r) {
        Image img;
        try {
            img = r.acquireLatestImage();
        } catch (IllegalStateException closed) {
            return;
        }
        if (img == null) {
            return;
        }
        try {
            Image.Plane y = img.getPlanes()[0];
            final String text = nativeDecode(y.getBuffer(), img.getWidth(), img.getHeight(), y.getRowStride());
            if (text != null) {
                later(text);
            }
        } finally {
            img.close();
        }
    }

    /** answer, from the camera's thread. (No lambdas: android.jar has no LambdaMetafactory for javac.) */
    private void later(final String text) {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                answer(text);
            }
        });
    }

    private void answer(String text) {
        if (answered) {
            return;
        }
        answered = true;
        nativeOnResult(text);
        finish();
    }

    @Override
    protected void onStop() {
        super.onStop();
        answer(null);
    }

    @Override
    protected void onDestroy() {
        super.onDestroy();
        answer(null);
        if (camera != null) {
            camera.close();
        }
        if (thread != null) {
            // On the camera's thread, after the frame being read: nativeDecode reads the reader's memory.
            final ImageReader r = reader;
            handler.post(new Runnable() {
                @Override
                public void run() {
                    if (r != null) {
                        r.close();
                    }
                }
            });
            thread.quitSafely();
        }
    }

    @Override
    public void onSurfaceTextureAvailable(SurfaceTexture t, int w, int h) {
        open();
    }

    @Override
    public void onSurfaceTextureSizeChanged(SurfaceTexture t, int w, int h) {
        if (size != null) {
            fit();
        }
    }

    @Override
    public boolean onSurfaceTextureDestroyed(SurfaceTexture t) {
        return true;
    }

    @Override
    public void onSurfaceTextureUpdated(SurfaceTexture t) {}

    /** The text of the QR code in a frame's luma plane, or null. */
    private native String nativeDecode(java.nio.ByteBuffer plane, int width, int height, int stride);

    /** The QR code's text, or null if the user went back. */
    private native void nativeOnResult(String text);
}
