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
import java.nio.ByteBuffer;
import java.util.Arrays;

public final class ScreenShareService extends Service {
    private MediaProjection projection;
    private VirtualDisplay display;
    private MediaCodec encoder;
    private Thread drainThread;
    private volatile boolean running;
    private final MediaProjection.Callback projectionCallback = new MediaProjection.Callback() {
        @Override public void onStop() { running = false; stopSelf(); }
    };
    private String sessionId;

    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent == null) { stopSelf(); return START_NOT_STICKY; }
        sessionId = intent.getStringExtra("session_id");
        int width = intent.getIntExtra("width", 1280);
        int height = intent.getIntExtra("height", 720);
        int fps = intent.getIntExtra("fps", 15);
        int density = intent.getIntExtra("density", getResources().getDisplayMetrics().densityDpi);
        int resultCode = intent.getIntExtra("result_code", Activity.RESULT_CANCELED);
        Intent data = Build.VERSION.SDK_INT >= 33
            ? intent.getParcelableExtra("projection_data", Intent.class)
            : intent.getParcelableExtra("projection_data");
        if (sessionId == null || resultCode != Activity.RESULT_OK || data == null) { stopSelf(); return START_NOT_STICKY; }

        NotificationManager manager = getSystemService(NotificationManager.class);
        manager.createNotificationChannel(new NotificationChannel("local-screen", "LoCAL screen sharing", NotificationManager.IMPORTANCE_LOW));
        PendingIntent open = PendingIntent.getActivity(this, 0, new Intent(this, MainActivity.class), PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        Notification notification = new Notification.Builder(this, "local-screen")
            .setContentTitle("LoCAL · 画面共有中")
            .setContentText("この端末の画面をLAN内のペア端末へ共有しています")
            .setSmallIcon(R.drawable.ic_local).setContentIntent(open).setOngoing(true).build();
        if (Build.VERSION.SDK_INT >= 29) startForeground(2, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION);
        else startForeground(2, notification);

        try {
            MediaProjectionManager mpm = getSystemService(MediaProjectionManager.class);
            projection = mpm.getMediaProjection(resultCode, data);
            projection.registerCallback(projectionCallback, new Handler(Looper.getMainLooper()));
            MediaFormat format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, width, height);
            format.setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface);
            format.setInteger(MediaFormat.KEY_BIT_RATE, Math.max(1_500_000, Math.min(6_000_000, width * height * fps / 3)));
            format.setInteger(MediaFormat.KEY_FRAME_RATE, fps);
            format.setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1);
            format.setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.AVCProfileBaseline);
            encoder = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC);
            encoder.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE);
            Surface surface = encoder.createInputSurface();
            encoder.start();
            display = projection.createVirtualDisplay("LoCAL screen", width, height, density,
                DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR, surface, null, null);
            running = true;
            drainThread = new Thread(this::drain, "LoCAL-screen-encoder");
            drainThread.start();
        } catch (Throwable e) {
            stopSelf();
        }
        return START_NOT_STICKY;
    }

    private void drain() {
        MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();
        byte[] codecConfig = null;
        long sequence = 0;
        try {
            while (running && encoder != null) {
                int index = encoder.dequeueOutputBuffer(info, 100_000);
                if (index == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED || index == MediaCodec.INFO_TRY_AGAIN_LATER) continue;
                if (index < 0) continue;
                ByteBuffer buffer = encoder.getOutputBuffer(index);
                if (buffer != null && info.size > 0) {
                    buffer.position(info.offset);
                    buffer.limit(info.offset + info.size);
                    byte[] raw = new byte[info.size];
                    buffer.get(raw);
                    byte[] annex = toAnnexB(raw);
                    boolean config = (info.flags & MediaCodec.BUFFER_FLAG_CODEC_CONFIG) != 0;
                    boolean key = (info.flags & MediaCodec.BUFFER_FLAG_KEY_FRAME) != 0;
                    if (config) {
                        codecConfig = annex;
                    } else {
                        byte[] frame = key && codecConfig != null ? concat(codecConfig, annex) : annex;
                        if (!Native.pushScreenFrame(sessionId, sequence++, Math.max(0L, info.presentationTimeUs), key, frame)) {
                            running = false;
                        }
                    }
                }
                encoder.releaseOutputBuffer(index, false);
                if ((info.flags & MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0) break;
            }
        } catch (Throwable ignored) {
        } finally {
            stopSelf();
        }
    }

    private static byte[] concat(byte[] a, byte[] b) {
        byte[] out = Arrays.copyOf(a, a.length + b.length);
        System.arraycopy(b, 0, out, a.length, b.length);
        return out;
    }

    private static byte[] toAnnexB(byte[] data) {
        if (data.length >= 4 && data[0] == 0 && data[1] == 0 && (data[2] == 1 || (data[2] == 0 && data[3] == 1))) return data;
        ByteBuffer in = ByteBuffer.wrap(data);
        java.io.ByteArrayOutputStream out = new java.io.ByteArrayOutputStream(data.length + 32);
        try {
            while (in.remaining() >= 4) {
                int length = in.getInt();
                if (length <= 0 || length > in.remaining()) return data;
                out.write(new byte[]{0,0,0,1});
                byte[] nal = new byte[length];
                in.get(nal);
                out.write(nal);
            }
            return out.size() > 0 ? out.toByteArray() : data;
        } catch (java.io.IOException impossible) {
            return data;
        }
    }

    @Override public void onDestroy() {
        running = false;
        if (display != null) { display.release(); display = null; }
        if (projection != null) {
            try { projection.unregisterCallback(projectionCallback); } catch (Throwable ignored) {}
            projection.stop(); projection = null;
        }
        if (encoder != null) {
            try { encoder.stop(); } catch (Throwable ignored) {}
            try { encoder.release(); } catch (Throwable ignored) {}
            encoder = null;
        }
        super.onDestroy();
    }

    @Override public IBinder onBind(Intent intent) { return null; }
}
