package org.sakus.local;

import android.app.*;
import android.content.*;
import android.content.pm.ServiceInfo;
import android.hardware.display.DisplayManager;
import android.hardware.display.VirtualDisplay;
import android.media.*;
import android.media.projection.MediaProjection;
import android.media.projection.MediaProjectionManager;
import android.os.*;
import android.view.Surface;
import java.io.ByteArrayOutputStream;
import java.nio.ByteBuffer;
import java.util.concurrent.*;
import org.json.JSONObject;

public final class ScreenShareService extends Service {
    static final String ACTION_START = "org.sakus.local.SCREEN_START";
    static final String ACTION_STOP = "org.sakus.local.SCREEN_STOP";
    static final String EXTRA_SESSION = "session";
    static final String EXTRA_WIDTH = "width";
    static final String EXTRA_HEIGHT = "height";
    static final String EXTRA_FPS = "fps";
    static final String EXTRA_RESULT_CODE = "resultCode";
    static final String EXTRA_RESULT_DATA = "resultData";
    private static final int CONTROL_KEYFRAME = 1, CONTROL_STOP = 2;

    private final ExecutorService worker = Executors.newSingleThreadExecutor();
    private volatile boolean running;
    private String sessionId;
    private MediaProjection projection;
    private MediaCodec codec;
    private VirtualDisplay display;
    private Surface surface;

    @Override public void onCreate() {
        super.onCreate();
        NotificationManager manager = getSystemService(NotificationManager.class);
        manager.createNotificationChannel(new NotificationChannel("local-screen", "LoCAL screen sharing", NotificationManager.IMPORTANCE_LOW));
    }

    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent == null || ACTION_STOP.equals(intent.getAction())) { stopSelf(); return START_NOT_STICKY; }
        if (!ACTION_START.equals(intent.getAction()) || running) { return START_NOT_STICKY; }
        sessionId = intent.getStringExtra(EXTRA_SESSION);
        Intent permission = intent.getParcelableExtra(EXTRA_RESULT_DATA);
        int resultCode = intent.getIntExtra(EXTRA_RESULT_CODE, Activity.RESULT_CANCELED);
        int width = intent.getIntExtra(EXTRA_WIDTH, 0);
        int height = intent.getIntExtra(EXTRA_HEIGHT, 0);
        int fps = intent.getIntExtra(EXTRA_FPS, 0);
        startScreenForeground();
        running = true;
        worker.execute(() -> runCapture(resultCode, permission, width, height, fps));
        return START_NOT_STICKY;
    }

    private void startScreenForeground() {
        PendingIntent open = PendingIntent.getActivity(this, 2, new Intent(this, MainActivity.class), PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        PendingIntent stop = PendingIntent.getService(this, 3, new Intent(this, ScreenShareService.class).setAction(ACTION_STOP), PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        Notification notification = new Notification.Builder(this, "local-screen")
            .setContentTitle("LoCAL · 画面共有中").setContentText("タップして開く / 停止で共有を終了")
            .setSmallIcon(R.drawable.ic_local).setContentIntent(open).setOngoing(true)
            .addAction(new Notification.Action.Builder(null, "停止", stop).build()).build();
        if (Build.VERSION.SDK_INT >= 29) { startForeground(2, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION); }
        else { startForeground(2, notification); }
    }

    private void runCapture(int resultCode, Intent permission, int width, int height, int fps) {
        try {
            if (sessionId == null || permission == null || width < 2 || height < 2 || fps < 1) { throw new IllegalArgumentException("Invalid screen capture session"); }
            MediaFormat format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, width, height);
            format.setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface);
            format.setInteger(MediaFormat.KEY_BIT_RATE, (int)Math.min(8_000_000L, Math.max(800_000L, (long)width * height * fps / 5L)));
            format.setInteger(MediaFormat.KEY_FRAME_RATE, fps);
            format.setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 2);
            format.setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.AVCProfileBaseline);
            codec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC);
            codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE);
            surface = codec.createInputSurface();
            codec.start();

            MediaProjectionManager manager = getSystemService(MediaProjectionManager.class);
            projection = manager.getMediaProjection(resultCode, permission);
            projection.registerCallback(new MediaProjection.Callback() {
                @Override public void onStop() { running = false; }
            }, new Handler(Looper.getMainLooper()));
            int density = getResources().getDisplayMetrics().densityDpi;
            display = projection.createVirtualDisplay("LoCAL-" + sessionId, width, height, density,
                DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR, surface, null, null);
            drainEncoder();
        } catch (Throwable error) {
            android.util.Log.e("LoCAL", "Screen capture failed", error);
        } finally {
            running = false;
            cleanup();
            stopNativeSession();
            stopSelf();
        }
    }

    private void drainEncoder() throws Exception {
        MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();
        byte[] codecConfig = new byte[0];
        long sequence = 0;
        while (running) {
            int control = Native.pollScreenControl(sessionId);
            if ((control & CONTROL_STOP) != 0) { break; }
            if ((control & CONTROL_KEYFRAME) != 0) {
                Bundle request = new Bundle();
                request.putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0);
                codec.setParameters(request);
            }
            int index = codec.dequeueOutputBuffer(info, 10_000);
            if (index == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                codecConfig = codecConfig(codec.getOutputFormat());
                continue;
            }
            if (index < 0) { continue; }
            ByteBuffer buffer = codec.getOutputBuffer(index);
            byte[] encoded = new byte[info.size];
            if (buffer != null && info.size > 0) {
                buffer.position(info.offset);
                buffer.limit(info.offset + info.size);
                buffer.get(encoded);
            }
            boolean config = (info.flags & MediaCodec.BUFFER_FLAG_CODEC_CONFIG) != 0;
            boolean keyframe = (info.flags & MediaCodec.BUFFER_FLAG_KEY_FRAME) != 0;
            codec.releaseOutputBuffer(index, false);
            if (config) { codecConfig = annexB(encoded); continue; }
            if (encoded.length == 0) { continue; }
            encoded = annexB(encoded);
            if (keyframe && codecConfig.length > 0) { encoded = join(codecConfig, encoded); }
            Native.pushScreenFrame(sessionId, encoded, sequence++, Math.max(0, info.presentationTimeUs), keyframe);
            if ((info.flags & MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0) { break; }
        }
    }

    private static byte[] codecConfig(MediaFormat format) {
        ByteArrayOutputStream output = new ByteArrayOutputStream();
        for (String key : new String[]{"csd-0", "csd-1"}) {
            ByteBuffer buffer = format.getByteBuffer(key);
            if (buffer == null) { continue; }
            ByteBuffer copy = buffer.duplicate();
            byte[] value = new byte[copy.remaining()];
            copy.get(value);
            try { output.write(annexB(value)); } catch (Exception ignored) {}
        }
        return output.toByteArray();
    }

    private static byte[] annexB(byte[] data) {
        if (data.length < 4 || (data[0] == 0 && data[1] == 0 && (data[2] == 1 || (data[2] == 0 && data[3] == 1)))) { return data; }
        ByteArrayOutputStream output = new ByteArrayOutputStream(data.length + 16);
        int offset = 0;
        while (offset + 4 <= data.length) {
            int length = ((data[offset] & 255) << 24) | ((data[offset + 1] & 255) << 16) | ((data[offset + 2] & 255) << 8) | (data[offset + 3] & 255);
            offset += 4;
            if (length <= 0 || offset + length > data.length) { return data; }
            output.write(0); output.write(0); output.write(0); output.write(1);
            output.write(data, offset, length);
            offset += length;
        }
        return offset == data.length ? output.toByteArray() : data;
    }

    private static byte[] join(byte[] first, byte[] second) {
        byte[] joined = new byte[first.length + second.length];
        System.arraycopy(first, 0, joined, 0, first.length);
        System.arraycopy(second, 0, joined, first.length, second.length);
        return joined;
    }

    private synchronized void cleanup() {
        if (display != null) { display.release(); display = null; }
        if (projection != null) { projection.stop(); projection = null; }
        if (codec != null) { try { codec.stop(); } catch (Throwable ignored) {} codec.release(); codec = null; }
        if (surface != null) { surface.release(); surface = null; }
    }

    private void stopNativeSession() {
        if (sessionId == null) { return; }
        try { Native.command(new JSONObject().put("op", "stop_screen").put("id", sessionId).toString()); } catch (Throwable ignored) {}
        sessionId = null;
    }

    @Override public void onDestroy() {
        running = false;
        worker.shutdownNow();
        cleanup();
        stopNativeSession();
        super.onDestroy();
    }

    @Override public IBinder onBind(Intent intent) { return null; }
}
