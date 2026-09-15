package org.sakus.local;

import android.app.*;
import android.content.*;
import android.net.wifi.WifiManager;
import android.os.*;
import java.io.File;
import java.util.concurrent.*;

public final class MeshService extends Service {
    static final ExecutorService WORK = Executors.newFixedThreadPool(4);
    static volatile String startupError;
    private WifiManager.MulticastLock multicast;

    @Override public void onCreate() {
        super.onCreate();
        NotificationManager manager = getSystemService(NotificationManager.class);
        manager.createNotificationChannel(new NotificationChannel("local", "LoCAL connection", NotificationManager.IMPORTANCE_LOW));
        PendingIntent open = PendingIntent.getActivity(this, 0, new Intent(this, MainActivity.class), PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        PendingIntent stop = PendingIntent.getService(this, 1, new Intent(this, MeshService.class).setAction("stop"), PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        Notification notification = new Notification.Builder(this, "local")
            .setContentTitle("LoCAL · LAN接続中")
            .setContentText("タップして開く / 停止で接続を終了")
            .setSmallIcon(org.sakus.local.R.drawable.ic_local).setContentIntent(open).setOngoing(true)
            .addAction(new Notification.Action.Builder(null, "停止", stop).build()).build();
        startForeground(1, notification);
        WifiManager wifi = (WifiManager)getApplicationContext().getSystemService(WIFI_SERVICE);
        if (wifi != null) { multicast = wifi.createMulticastLock("LoCAL discovery"); multicast.setReferenceCounted(false); multicast.acquire(); }
        WORK.execute(() -> {
            try {
                org.json.JSONObject result = new org.json.JSONObject(Native.start(new File(getFilesDir(), "mesh").getAbsolutePath(), new File(getFilesDir(), "received").getAbsolutePath(), Build.MODEL));
                startupError = result.optBoolean("ok") ? null : result.optString("error");
            } catch (Throwable e) { startupError = e.toString(); }
        });
    }
    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent != null && "stop".equals(intent.getAction())) { stopSelf(); }
        return START_NOT_STICKY;
    }
    // Android 15 limits long-running dataSync foreground services.
    @Override public void onTimeout(int startId, int fgsType) { stopSelf(); }
    @Override public void onDestroy() {
        Native.stop();
        if (multicast != null && multicast.isHeld()) { multicast.release(); }
        super.onDestroy();
    }
    @Override public IBinder onBind(Intent intent) { return null; }
}

