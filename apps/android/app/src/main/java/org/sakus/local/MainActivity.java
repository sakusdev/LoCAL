package org.sakus.local;

import android.app.Activity;
import android.content.*;
import android.database.Cursor;
import android.net.Uri;
import android.os.*;
import android.provider.OpenableColumns;
import android.webkit.*;
import android.widget.Toast;
import org.json.JSONObject;
import java.io.*;
import java.util.UUID;

public final class MainActivity extends Activity {
    private WebView web;
    private String pickerId, pickerPeer, exportId;
    private File exportSource;
    private static final int PICK = 101, EXPORT = 102;

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        startForegroundService(new Intent(this, MeshService.class));
        if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission("android.permission.POST_NOTIFICATIONS") != android.content.pm.PackageManager.PERMISSION_GRANTED) {
            requestPermissions(new String[]{"android.permission.POST_NOTIFICATIONS"}, 103);
        }
        web = new WebView(this);
        web.setBackgroundColor(0xff090d12);
        web.getSettings().setJavaScriptEnabled(true);
        web.getSettings().setDomStorageEnabled(true);
        web.getSettings().setAllowFileAccess(false);
        web.getSettings().setAllowContentAccess(false);
        web.getSettings().setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
        web.addJavascriptInterface(new Bridge(), "LocalNative");
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
                            startActivityForResult(new Intent(Intent.ACTION_OPEN_DOCUMENT).setType("*/*").addCategory(Intent.CATEGORY_OPENABLE), PICK);
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
                    } else if ("read_clipboard".equals(op) || "write_clipboard".equals(op)) {
                        runOnUiThread(() -> {
                            ClipboardManager manager = (ClipboardManager)getSystemService(CLIPBOARD_SERVICE);
                            if ("write_clipboard".equals(op)) { manager.setPrimaryClip(ClipData.newPlainText("LoCAL", request.optString("text"))); success(id, ""); }
                            else { ClipData clip = manager.getPrimaryClip(); success(id, clip != null && clip.getItemCount() > 0 ? clip.getItemAt(0).coerceToText(MainActivity.this).toString() : ""); }
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
        if (request == PICK) {
            String id = pickerId, peer = pickerPeer; pickerId = null; pickerPeer = null;
            if (id == null) { return; }
            if (result != RESULT_OK || data == null || data.getData() == null) { success(id, JSONObject.NULL); return; }
            Uri uri = data.getData();
            MeshService.WORK.execute(() -> {
                try {
                    String name = "shared-file";
                    try (Cursor cursor = getContentResolver().query(uri, new String[]{OpenableColumns.DISPLAY_NAME}, null, null, null)) { if (cursor != null && cursor.moveToFirst()) { name = cursor.getString(0); } }
                    if (name == null || name.isEmpty()) { name = "shared-file"; }
                    name = name.replaceAll("[<>:\"/\\\\|?*\\p{Cntrl}]", "_");
                    File directory = new File(getCacheDir(), "outgoing-" + UUID.randomUUID());
                    if (!directory.mkdirs()) { throw new IOException("Cannot create outgoing cache"); }
                    File file = new File(directory, name);
                    try (InputStream input = getContentResolver().openInputStream(uri); OutputStream output = new FileOutputStream(file)) { copy(input, output, 20L * 1024 * 1024 * 1024); }
                    reply(id, Native.command(new JSONObject().put("op", "send_file").put("peer_id", peer).put("path", file.getAbsolutePath()).toString()));
                } catch (Throwable e) { fail(id, e.toString()); }
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
    private static void copy(InputStream input, OutputStream output, long limit) throws IOException {
        if (input == null || output == null) { throw new IOException("Cannot open document"); }
        byte[] buffer = new byte[1024 * 1024]; long total = 0; int n;
        while ((n = input.read(buffer)) != -1) { total += n; if (total > limit) { throw new IOException("File exceeds 20 GiB"); } output.write(buffer, 0, n); }
    }
    @Override protected void onDestroy() { if (web != null) { web.removeJavascriptInterface("LocalNative"); web.destroy(); web = null; } super.onDestroy(); }
}

