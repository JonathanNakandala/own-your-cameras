package org.ownyourcameras.ds_kh9510_wte1;

import android.app.Activity;
import android.app.AlertDialog;
import android.os.Bundle;
import android.view.View;
import android.content.DialogInterface;
import android.net.LocalSocket;
import android.net.LocalSocketAddress;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.TextView;
import java.io.*;

/** Explicit, local-only actions; no listener, background service or telemetry. */
public class MainActivity extends Activity {
    private TextView result;
    private Button probe, install;
    private boolean rootChecked;
    private boolean uncertain;

    public void onCreate(Bundle state) {
        super.onCreate(state);
        LinearLayout layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        result = new TextView(this);
        result.setText("DS-KH9510-WTE1 / V2.2.28 build 230613\nCheck root first.");
        probe = new Button(this);
        probe.setText("Check root (id)");
        install = new Button(this);
        install.setText("Install bundled SSH public key");
        install.setEnabled(false);
        layout.addView(probe); layout.addView(install); layout.addView(result);
        setContentView(layout);
        probe.setOnClickListener(new View.OnClickListener() {
            public void onClick(View v) { execute(false); }
        });
        install.setOnClickListener(new View.OnClickListener() {
            public void onClick(View v) {
                new AlertDialog.Builder(MainActivity.this)
                    .setMessage("Write the bundled key to /system/etc/dropbear/.ssh/authorized_keys? "
                        + "Existing keys are preserved. This temporarily makes /system writable.")
                    .setNegativeButton("Cancel", null)
                    .setPositiveButton("Install", new DialogInterface.OnClickListener() {
                        public void onClick(DialogInterface dialog, int which) { execute(true); }
                    }).show();
            }
        });
    }

    private void execute(final boolean writeKey) {
        probe.setEnabled(false); install.setEnabled(false);
        result.setText("Waiting for root command…");
        new Thread(new Runnable() { public void run() {
            String message;
            try { message = MainActivity.this.run(writeKey); }
            catch (Exception e) { message = "Error: " + e.getMessage(); uncertain = true; }
            final String text = message;
            runOnUiThread(new Runnable() { public void run() {
                if (text.startsWith("Timed out:")) uncertain = true;
                if (!writeKey) rootChecked = text.contains("uid=0(root)");
                result.setText(text);
                probe.setEnabled(!uncertain); install.setEnabled(rootChecked && !uncertain);
            }});
        }}).start();
    }

    private synchronized String run(boolean writeKey) throws Exception {
        File script = new File(getFilesDir(), "action.sh");
        File output = new File(getFilesDir(), "result.txt");
        File done = new File(getFilesDir(), "done.txt");
        done.delete();
        try (FileOutputStream f = new FileOutputStream(output)) { /* app-owned result */ }
        // Root must redirect into an app-owned sentinel so the app can read it.
        try (FileOutputStream f = new FileOutputStream(done)) { }
        if (writeKey) {
            copyAsset("install-key.sh", script);
            copyAsset("owner.pub", new File(getFilesDir(), "owner.pub"));
        } else {
            try (FileOutputStream f = new FileOutputStream(script)) {
                f.write("/system/bin/id\n".getBytes("UTF-8"));
            }
        }
        String base = getFilesDir().getAbsolutePath();
        String command = "/system/bin/sh '" + script + "' '" + base
            + "/owner.pub' > '" + output + "' 2>&1; echo $? > '" + done + "'";
        try (LocalSocket socket = new LocalSocket()) {
            socket.connect(new LocalSocketAddress("cliserver", LocalSocketAddress.Namespace.ABSTRACT));
            socket.getOutputStream().write(command.getBytes("UTF-8"));
            socket.getOutputStream().flush();
            for (int n = 0; n < 150; n++) {
                if (done.length() > 0) return read(output) + "\nExit: " + read(done);
                Thread.sleep(100);
            }
        }
        return "Timed out: command state is unknown. Do not retry a write blindly.\n" + read(output);
    }

    private void copyAsset(String name, File target) throws IOException {
        try (InputStream in = getAssets().open(name); FileOutputStream out = new FileOutputStream(target)) {
            byte[] buffer = new byte[4096]; int n;
            while ((n = in.read(buffer)) != -1) out.write(buffer, 0, n);
        }
    }
    private String read(File file) throws IOException {
        try (FileInputStream in = new FileInputStream(file)) {
            byte[] buffer = new byte[16384]; int n = in.read(buffer);
            return n < 0 ? "" : new String(buffer, 0, n, "UTF-8");
        }
    }
}
