package org.decented.gameflock;

import android.app.NativeActivity;
import android.content.Intent;
import android.os.Bundle;
import dev.forgesworn.release.Updater;

/**
 * NativeActivity that keeps the newest intent. Stock NativeActivity drops
 * onNewIntent, so a link opened while the game runs (singleTask) would leave
 * getIntent() on the launch intent. The native side reads getIntent() over JNI
 * (app/src/platform/android.rs) and consumes the link with setData(null).
 *
 * It also hands itself to BtBridge (Bluetooth games need an Activity to ask
 * for permissions) and forwards the permission answer, and hands itself to
 * {@link Updater} (the release kit's: in-app updates and the More games installs need an Activity
 * to show Android's install prompt) and tells it when the game comes back to
 * the front.
 */
public class FlockActivity extends NativeActivity {
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        BtBridge.attach(this);
        Updater.attach(this);
        super.onCreate(savedInstanceState);
    }

    @Override
    protected void onResume() {
        super.onResume();
        Updater.onResume();
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions, int[] grantResults) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults);
        BtBridge.onPermissionsResult(this, requestCode, permissions, grantResults);
    }
}
