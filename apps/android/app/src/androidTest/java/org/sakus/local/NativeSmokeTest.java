package org.sakus.local;

import android.content.Context;
import android.content.Intent;
import android.os.SystemClock;
import androidx.test.platform.app.InstrumentationRegistry;
import org.junit.Test;
import static org.junit.Assert.*;
import org.json.*;
import java.io.*;

public final class NativeSmokeTest {
    private JSONObject call(JSONObject request) throws Exception {
        JSONObject response = new JSONObject(Native.command(request.toString()));
        assertTrue(response.toString(), response.getBoolean("ok"));
        return response.getJSONObject("data");
    }
    private JSONObject state() throws Exception { return call(new JSONObject().put("op", "snapshot")); }
    @Test public void testAndroidLinuxInteroperability() throws Exception {
        Context context = InstrumentationRegistry.getInstrumentation().getTargetContext();
        context.startActivity(new Intent(context, MainActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
        boolean ready = false;
        for (int i=0;i<100;i++) {
            if (new JSONObject(Native.command("{\"op\":\"snapshot\"}")).optBoolean("ok")) {ready=true;break;}
            SystemClock.sleep(100);
        }
        assertTrue("Native Android engine must start",ready);
        String peerId = call(new JSONObject().put("op","connect").put("address","10.0.2.2:53319")).getString("id");
        String code = state().getJSONArray("peers").getJSONObject(0).getString("code");
        assertEquals(6,code.length());
        call(new JSONObject().put("op","confirm").put("peer_id",peerId).put("code",code));
        for (int i=0;i<100&&!state().getJSONArray("peers").getJSONObject(0).getBoolean("ready");i++) {SystemClock.sleep(100);}
        assertTrue(state().getJSONArray("peers").getJSONObject(0).getBoolean("ready"));
        call(new JSONObject().put("op","send_text").put("peer_id",peerId).put("text","こんにちは Linux from Android 👋"));
        boolean received = false;
        for (int i=0;i<200&&!received;i++) {
            JSONArray transfers=state().getJSONArray("transfers");
            for(int j=0;j<transfers.length();j++) {
                JSONObject transfer=transfers.getJSONObject(j);
                if("offered".equals(transfer.getString("status"))) {call(new JSONObject().put("op","accept_file").put("id",transfer.getString("id")).put("accept",true));}
                if("completed".equals(transfer.getString("status"))&&"in".equals(transfer.getString("direction"))) {
                    try(InputStream input=new BufferedInputStream(new FileInputStream(transfer.getString("path")))) {for(int n=0;n<2500007;n++){assertEquals(n%251,input.read());}assertEquals(-1,input.read());}
                    received=true;
                }
            }
            SystemClock.sleep(100);
        }
        assertTrue("Linux -> Android file must arrive with exact content",received);
        assertTrue(state().getJSONArray("messages").toString().contains("Hello Android from Linux"));
        File outgoing=new File(context.getCacheDir(),"from-android.bin");
        try(OutputStream output=new BufferedOutputStream(new FileOutputStream(outgoing))) {for(int n=0;n<2500007;n++){output.write(n%251);}}
        String sendId=call(new JSONObject().put("op","send_file").put("peer_id",peerId).put("path",outgoing.getAbsolutePath())).getString("id");
        boolean sent=false;
        for(int i=0;i<200&&!sent;i++) {
            JSONArray transfers=state().getJSONArray("transfers");
            for(int j=0;j<transfers.length();j++){JSONObject t=transfers.getJSONObject(j);if(sendId.equals(t.getString("id"))&&"completed".equals(t.getString("status"))){sent=true;}}
            SystemClock.sleep(100);
        }
        assertTrue("Android -> Linux file must be acknowledged",sent);
    }
}
