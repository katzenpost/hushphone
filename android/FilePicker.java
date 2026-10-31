// SPDX-License-Identifier: AGPL-3.0-only
package network.katzenpost.hushphone;

import android.app.Activity;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;

/**
 * A see-through activity that opens the system's document picker for the app and hands its
 * answer to the native code. NativeActivity has no onActivityResult, so the picker cannot be
 * opened from it; this activity is started in its place, opens the picker for a result, passes
 * the document's URI to nativeOnResult, and finishes.
 *
 * The intent that starts it carries "mode": "open" to pick a document to read (the user's file
 * to send) or "create" to pick where to write one (a received file to save), and "name", the
 * name to suggest when creating. The app gets the one document the user chose, and nothing else.
 */
public final class FilePicker extends Activity {
    static {
        // JNI looks the native method up in the libraries loaded by this class's loader; the
        // app's library was loaded by the framework's for NativeActivity. Loading it here again
        // costs nothing and makes the lookup succeed.
        System.loadLibrary("hushphone");
    }

    private static final int REQUEST = 1;

    @Override
    protected void onCreate(Bundle saved) {
        super.onCreate(saved);
        if (saved != null) {
            // Brought back after being destroyed: the picker is gone with its answer.
            nativeOnResult(null);
            finish();
            return;
        }
        Intent from = getIntent();
        Intent pick;
        if ("create".equals(from.getStringExtra("mode"))) {
            pick = new Intent(Intent.ACTION_CREATE_DOCUMENT);
            pick.addCategory(Intent.CATEGORY_OPENABLE);
            pick.setType("application/octet-stream");
            pick.putExtra(Intent.EXTRA_TITLE, from.getStringExtra("name"));
        } else {
            pick = new Intent(Intent.ACTION_OPEN_DOCUMENT);
            pick.addCategory(Intent.CATEGORY_OPENABLE);
            pick.setType("*/*");
        }
        try {
            startActivityForResult(pick, REQUEST);
        } catch (android.content.ActivityNotFoundException e) {
            nativeOnResult(null);
            finish();
        }
    }

    @Override
    protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        String uri = null;
        if (request == REQUEST && result == RESULT_OK && data != null) {
            Uri u = data.getData();
            if (u != null) {
                uri = u.toString();
            }
        }
        nativeOnResult(uri);
        finish();
    }

    /** The document's URI, or null if the user went back. */
    private native void nativeOnResult(String uri);
}
