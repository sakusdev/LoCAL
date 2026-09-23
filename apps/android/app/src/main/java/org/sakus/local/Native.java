package org.sakus.local;

final class Native {
    static { System.loadLibrary("local_android"); }
    static native String start(String data, String received, String name);
    static native String command(String request);
    static native boolean pushScreenFrame(String id, byte[] data, long sequence, long timestampUs, boolean keyframe);
    static native int pollScreenControl(String id);
    static native void stop();
    private Native() {}
}
