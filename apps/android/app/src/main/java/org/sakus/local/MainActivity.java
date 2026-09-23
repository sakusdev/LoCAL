package org.sakus.local;

import android.app.Activity;
import android.content.*;
import android.database.Cursor;
import android.net.Uri;
import android.media.projection.MediaProjectionManager;
import android.os.*;
import android.provider.OpenableColumns;
import android.webkit.*;
import android.widget.Toast;
import org.json.JSONArray;
import org.json.JSONObject;
import java.io.*;
import java.util.ArrayList;
import java.util.UUID;

public final class MainActivity extends Activity {
    private WebView web;
    private String pickerId, pickerPeer, exportId;
    private File exportSource;
    private PermissionRequest microphoneRequest;
    private ClipboardManager clipboardManager;
    private ClipboardManager.OnPrimaryClipChangedListener clipboardListener;
    private volatile long clipboardRevision;
    private String screenCallId;
    private JSONObject screenSession;
    private static final int PICK = 101, EXPORT = 102, MICROPHONE = 104, SCREEN_CAPTURE = 105;

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        startForegroundService(new Intent(this, MeshService.class));
        if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission("android.permission.POST_NOTIFICATIONS") != android.content.pm.PackageManager.PERMISSION_GRANTED) {
            requestPermissions(new String[]{"android.permission.POST_NOTIFICATIONS"}, 103);
        }
        web = new WebView(this);
        clipboardManager = (ClipboardManager)getSystemService(CLIPBOARD_SERVICE);
        clipboardListener = () -> clipboardRevision++;
        clipboardManager.addPrimaryClipChangedListener(clipboardListener);
        web.setBackgroundColor(0xfff1eee6);
        web.getSettings().setJavaScriptEnabled(true);
        web.getSettings().setDomStorageEnabled(true);
        web.getSettings().setAllowFileAccess(false);
        web.getSettings().setAllowContentAccess(false);
        web.getSettings().setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
        web.addJavascriptInterface(new Bridge(), "LocalNative");
        web.setWebChromeClient(new WebChromeClient() {
            @Override public void onPermissionRequest(PermissionRequest request) {
                runOnUiThread(() -> {
                    Uri origin = request.getOrigin();
                    boolean local = origin != null && "https".equals(origin.getScheme()) && "local.app".equals(origin.getHost());
                    boolean audioOnly = request.getResources().length == 1 && PermissionRequest.RESOURCE_AUDIO_CAPTURE.equals(request.getResources()[0]);
                    if (!local || !audioOnly || microphoneRequest != null) { request.deny(); return; }
                    if (Build.VERSION.SDK_INT < 23 || checkSelfPermission(android.Manifest.permission.RECORD_AUDIO) == android.content.pm.PackageManager.PERMISSION_GRANTED) {
                        request.grant(new String[]{PermissionRequest.RESOURCE_AUDIO_CAPTURE});
                    } else {
                        microphoneRequest = request;
                        requestPermissions(new String[]{android.Manifest.permission.RECORD_AUDIO}, MICROPHONE);
                    }
                });
            }
            @Override public void onPermissionRequestCanceled(PermissionRequest request) {
                if (microphoneRequest == request) { microphoneRequest = null; }
            }
        });
        web.setWebViewClient(new WebViewClient() {
            @Override public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) { return true; }
            @Override public WebResourceResponse shouldInterceptRequest(WebView view, WebResourceRequest request) {
                Uri uri = request.getUrl();
                if (!"https".equals(uri.getScheme()) || !"local.app".equals(uri.getHost())) {
                    return new WebResourceResponse("text/plain", "UTF-8", new ByteArrayInputStream(new byte[0]));
                }
                String path = uri.getPath();
                if (path == null || path.equals("/")) { path = "/index.html"; }
                if (!path.matches("/[a-zA-Z0-9._-]+") || path.contains("..")) { return new WebResourceResponse("text/plain", "UTF-8", new ByteArrayInputStream(new byte[0])); }
                String mime = path.endsWith(".js") ? "text/javascript" : path.endsWith(".css") ? "text/css" : path.endsWith(".svg") ? "image/svg+xml" : "text/html";
                try { return new WebResourceResponse(mime, "UTF-8", getAssets().open(path.substring(1))); }
                catch (IOException e) { return new WebResourceResponse("text/plain", "UTF-8", new ByteArrayInputStream(new byte[0])); }
            }
        });
        setContentView(web);
        web.loadUrl("https://local.app/index.html");
    }
    private void reply(String id, String json) {
        runOnUiThread(() -> { if (web != null) { web.evaluateJavascript("window.localResolve(" + JSONObject.quote(id) + "," + JSONObject.quote(json) + ")", null); } });
    }
    private void fail(String id, String error) { try { reply(id, new JSONObject().put("ok", false).put("error", error).toString()); } catch (Exception ignored) {} }
    private void success(String id, Object data) { try { reply(id, new JSONObject().put("ok", true).put("data", data).toString()); } catch (Exception ignored) {} }

    final class Bridge {
        @JavascriptInterface public void invoke(String id, String body) {
            MeshService.WORK.execute(() -> {
                try {
                    JSONObject request = new JSONObject(body);
                    String op = request.getString("op");
                    if ("pick_file".equals(op)) {
                        runOnUiThread(() -> {
                            if (pickerId != null) { fail(id, "ファイル選択中です"); return; }
                            pickerId = id; pickerPeer = request.optString("peer_id");
                            startActivityForResult(new Intent(Intent.ACTION_OPEN_DOCUMENT).setType("*/*").addCategory(Intent.CATEGORY_OPENABLE).putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true), PICK);
                        });
                    } else if ("export_file".equals(op)) {
                        File source = new File(request.getString("path")).getCanonicalFile();
                        File root = new File(getFilesDir(), "received").getCanonicalFile();
                        if (!source.isFile() || !root.equals(source.getParentFile())) { throw new IOException("Invalid received file"); }
                        runOnUiThread(() -> {
                            if (exportId != null) { fail(id, "保存先を選択中です"); return; }
                            exportId = id; exportSource = source;
                            startActivityForResult(new Intent(Intent.ACTION_CREATE_DOCUMENT).setType("application/octet-stream").addCategory(Intent.CATEGORY_OPENABLE).putExtra(Intent.EXTRA_TITLE, request.optString("name", source.getName())), EXPORT);
                        });
                    } else if ("clipboard_revision".equals(op)) {
                        success(id, clipboardRevision);
                    } else if ("read_clipboard".equals(op) || "write_clipboard".equals(op)) {
                        runOnUiThread(() -> {
                            if ("write_clipboard".equals(op)) { clipboardManager.setPrimaryClip(ClipData.newPlainText("LoCAL", request.optString("text"))); success(id, ""); }
                            else { ClipData clip = clipboardManager.getPrimaryClip(); success(id, clip != null && clip.getItemCount() > 0 ? clip.getItemAt(0).coerceToText(MainActivity.this).toString() : ""); }
                        });
                    } else if ("screen_sources".equals(op)) {
                        runOnUiThread(() -> {
                            android.util.DisplayMetrics metrics = getResources().getDisplayMetrics();
                            try {
                                JSONObject source = new JSONObject().put("id", "display-0").put("name", "このAndroid画面")
                                    .put("kind", "display").put("width", metrics.widthPixels).put("height", metrics.heightPixels).put("primary", true);
                                success(id, new JSONObject().put("backend", "android-mediaprojection-mediacodec").put("sources", new JSONArray().put(source)));
                            } catch (Throwable error) { fail(id, error.toString()); }
                        });
                    } else if ("share_screen".equals(op)) {
                        if (!"display-0".equals(request.optString("source_id"))) { throw new IOException("Android screen source is unavailable"); }
                        android.util.DisplayMetrics metrics = getResources().getDisplayMetrics();
                        request.put("source_width", metrics.widthPixels).put("source_height", metrics.heightPixels);
                        if (MeshService.startupError != null) { throw new IOException(MeshService.startupError); }
                        JSONObject response = new JSONObject(Native.command(request.toString()));
                        JSONObject responseData = response.optJSONObject("data");
                        if (!response.optBoolean("ok") || responseData == null || !responseData.optBoolean("accepted")) { reply(id, response.toString()); return; }
                        runOnUiThread(() -> {
                            if (screenCallId != null) {
                                try { Native.command(new JSONObject().put("op", "stop_screen").put("id", responseData.optString("id")).toString()); } catch (Throwable ignored) {}
                                fail(id, "画面共有の許可を確認中です");
                                return;
                            }
                            screenCallId = id;
                            screenSession = responseData;
                            MediaProjectionManager manager = getSystemService(MediaProjectionManager.class);
                            startActivityForResult(manager.createScreenCaptureIntent(), SCREEN_CAPTURE);
                        });
                    } else {
                        if (MeshService.startupError != null) { throw new IOException(MeshService.startupError); }
                        reply(id, Native.command(body));
                    }
                } catch (Throwable e) { fail(id, e.getMessage() == null ? e.toString() : e.getMessage()); }
            });
        }
    }
    @Override protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request == SCREEN_CAPTURE) {
            String callId = screenCallId;
            JSONObject session = screenSession;
            screenCallId = null;
            screenSession = null;
            if (callId == null || session == null) { return; }
            String sessionId = session.optString("id");
            if (result != RESULT_OK || data == null) {
                MeshService.WORK.execute(() -> { try { Native.command(new JSONObject().put("op", "stop_screen").put("id", sessionId).toString()); } catch (Throwable ignored) {} });
                try { session.put("accepted", false).put("cancelled", true); } catch (Throwable ignored) {}
                success(callId, session);
                return;
            }
            JSONObject profile = session.optJSONObject("profile");
            if (sessionId.isEmpty() || profile == null || profile.optInt("width") < 2 || profile.optInt("height") < 2 || profile.optInt("fps") < 1) {
                MeshService.WORK.execute(() -> { try { Native.command(new JSONObject().put("op", "stop_screen").put("id", sessionId).toString()); } catch (Throwable ignored) {} });
                fail(callId, "画面共有の設定を開始できませんでした");
                return;
            }
            Intent capture = new Intent(this, ScreenShareService.class).setAction(ScreenShareService.ACTION_START)
                .putExtra(ScreenShareService.EXTRA_SESSION, sessionId)
                .putExtra(ScreenShareService.EXTRA_WIDTH, profile.optInt("width"))
                .putExtra(ScreenShareService.EXTRA_HEIGHT, profile.optInt("height"))
                .putExtra(ScreenShareService.EXTRA_FPS, profile.optInt("fps"))
                .putExtra(ScreenShareService.EXTRA_RESULT_CODE, result)
                .putExtra(ScreenShareService.EXTRA_RESULT_DATA, data);
            try {
                if (Build.VERSION.SDK_INT >= 26) { startForegroundService(capture); } else { startService(capture); }
                success(callId, session);
            } catch (Throwable error) {
                MeshService.WORK.execute(() -> { try { Native.command(new JSONObject().put("op", "stop_screen").put("id", sessionId).toString()); } catch (Throwable ignored) {} });
                fail(callId, error.getMessage() == null ? "画面共有を開始できませんでした" : error.getMessage());
            }
        } else if (request == PICK) {
            String id = pickerId, peer = pickerPeer; pickerId = null; pickerPeer = null;
            if (id == null) { return; }
            if (result != RESULT_OK || data == null) { success(id, JSONObject.NULL); return; }
            ArrayList<Uri> uris = new ArrayList<>();
            if (data.getClipData() != null) {
                ClipData clips = data.getClipData();
                if (clips.getItemCount() > 8) { fail(id, "一度に送信できるのは8件までです。"); return; }
                for (int i = 0; i < clips.getItemCount(); i++) { uris.add(clips.getItemAt(i).getUri()); }
            } else if (data.getData() != null) { uris.add(data.getData()); }
            if (uris.isEmpty()) { success(id, JSONObject.NULL); return; }
            MeshService.WORK.execute(() -> {
                JSONArray ids = new JSONArray();
                try {
                    for (Uri uri : uris) {
                        String name = "shared-file";
                        try (Cursor cursor = getContentResolver().query(uri, new String[]{OpenableColumns.DISPLAY_NAME}, null, null, null)) { if (cursor != null && cursor.moveToFirst()) { name = cursor.getString(0); } }
                        if (name == null || name.isEmpty()) { name = "shared-file"; }
                        name = name.replaceAll("[<>:\"/\\\\|?*\\p{Cntrl}]", "_");
                        File directory = new File(getCacheDir(), "outgoing-" + UUID.randomUUID());
                        if (!directory.mkdirs()) { throw new IOException("Cannot create outgoing cache"); }
                        File file = new File(directory, name);
                        try {
                            try (InputStream input = getContentResolver().openInputStream(uri); OutputStream output = new FileOutputStream(file)) { copy(input, output, 20L * 1024 * 1024 * 1024); }
                            JSONObject response = new JSONObject(Native.command(new JSONObject().put("op", "send_file").put("peer_id", peer).put("path", file.getAbsolutePath()).toString()));
                            if (!response.optBoolean("ok")) { throw new IOException(response.optString("error", "Cannot send file")); }
                            ids.put(response.getJSONObject("data").getString("id"));
                        } catch (Throwable e) { file.delete(); directory.delete(); throw e; }
                    }
                    success(id, new JSONObject().put("ids", ids));
                } catch (Throwable e) {
                    if (ids.length() == 0) { fail(id, e.getMessage() == null ? e.toString() : e.getMessage()); }
                    else { try { success(id, new JSONObject().put("ids", ids).put("error", e.getMessage() == null ? e.toString() : e.getMessage())); } catch (Exception ignored) {} }
                }
            });
        } else if (request == EXPORT) {
            String id = exportId; File source = exportSource; exportId = null; exportSource = null;
            if (id == null) { return; }
            if (result != RESULT_OK || data == null || data.getData() == null) { success(id, JSONObject.NULL); return; }
            Uri uri = data.getData();
            MeshService.WORK.execute(() -> {
                try (InputStream input = new FileInputStream(source); OutputStream output = getContentResolver().openOutputStream(uri)) { copy(input, output, Long.MAX_VALUE); success(id, true); }
                catch (Throwable e) { fail(id, e.toString()); }
            });
        }
    }
    @Override public void onRequestPermissionsResult(int requestCode, String[] permissions, int[] results) {
        super.onRequestPermissionsResult(requestCode, permissions, results);
        if (requestCode != MICROPHONE) { return; }
        PermissionRequest request = microphoneRequest;
        microphoneRequest = null;
        if (request == null) { return; }
        if (results.length > 0 && results[0] == android.content.pm.PackageManager.PERMISSION_GRANTED) {
            request.grant(new String[]{PermissionRequest.RESOURCE_AUDIO_CAPTURE});
        } else { request.deny(); }
    }
    private static void copy(InputStream input, OutputStream output, long limit) throws IOException {
        if (input == null || output == null) { throw new IOException("Cannot open document"); }
        byte[] buffer = new byte[1024 * 1024]; long total = 0; int n;
        while ((n = input.read(buffer)) != -1) { total += n; if (total > limit) { throw new IOException("File exceeds 20 GiB"); } output.write(buffer, 0, n); }
    }
    @Override protected void onDestroy() { if (microphoneRequest != null) { microphoneRequest.deny(); microphoneRequest = null; } if (clipboardManager != null && clipboardListener != null) { clipboardManager.removePrimaryClipChangedListener(clipboardListener); } if (web != null) { web.removeJavascriptInterface("LocalNative"); web.destroy(); web = null; } super.onDestroy(); }
}
