package org.sakus.local;

final class Native {
    static { System.loadLibrary("local_android"); }
    static native String start(String data, String received, String name);
    static native String command(String request);
    static native void stop();
    private Native() {}
}

