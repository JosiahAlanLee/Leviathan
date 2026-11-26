package io.github.josiahalanlee.leviathan;

import android.app.Activity;
import android.app.NativeActivity;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.provider.OpenableColumns;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;

/** The Rust engine stays in NativeActivity; only the document result crosses here. */
public final class DocumentActivity extends NativeActivity {
    private static final int BASE = 4700, WAV = 1, OPEN = 2, SAVE = 3;
    private static final int WAV_LIMIT = 64 * 1024 * 1024, PROJECT_LIMIT = 8 * 1024 * 1024;
    private int operation;
    private byte[] saveBytes;
    private volatile String result;

    public void launchDocument(int kind, byte[] payload) {
        if (kind < WAV || kind > SAVE || operation != 0) return;
        result = null;
        operation = kind;
        saveBytes = payload;
        Intent intent = new Intent(kind == SAVE ? Intent.ACTION_CREATE_DOCUMENT : Intent.ACTION_OPEN_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        if (kind == SAVE) {
            intent.setType("application/json");
            intent.putExtra(Intent.EXTRA_TITLE, "untitled.leviathan.json");
        } else {
            // Several providers advertise WAV and JSON as octet-stream. Rust validates bytes.
            intent.setType("*/*");
        }
        try {
            startActivityForResult(intent, BASE + kind);
        } catch (RuntimeException error) {
            saveBytes = null;
            result = "error\nCould not open Android Files: " + error.getMessage();
        }
    }

    // Called by Rust on the Java main looper; the worker publishes result once.
    public String takeDocumentResult() {
        String ready = result;
        if (ready != null) {
            result = null;
            operation = 0;
        }
        return ready;
    }

    @Override protected void onActivityResult(int code, int status, Intent intent) {
        super.onActivityResult(code, status, intent);
        final int kind = code - BASE;
        if (kind == 0 || kind != operation) return;
        final byte[] output = saveBytes;
        saveBytes = null;
        final Uri uri = intent == null ? null : intent.getData();
        if (status != Activity.RESULT_OK || uri == null) {
            result = "cancel";
            return;
        }

        new Thread(new Runnable() {
            @Override public void run() {
            File temporary = null;
            try {
                if (kind == SAVE) {
                    if (output == null || output.length > PROJECT_LIMIT) throw new IOException("Invalid project bytes");
                    try (OutputStream stream = getContentResolver().openOutputStream(uri, "w")) {
                        if (stream == null) throw new IOException("Files did not provide a writable document");
                        stream.write(output);
                    }
                    result = "saved";
                } else {
                    String name = displayName(uri, kind == WAV ? "sample.wav" : "project.leviathan.json");
                    if (name.length() > 80) name = name.substring(0, 80);
                    temporary = File.createTempFile("leviathan-", ".import", getCacheDir());
                    try (InputStream stream = getContentResolver().openInputStream(uri);
                         FileOutputStream local = new FileOutputStream(temporary)) {
                        if (stream == null) throw new IOException("Files did not provide a readable document");
                        int limit = kind == WAV ? WAV_LIMIT : PROJECT_LIMIT;
                        byte[] buffer = new byte[16384];
                        int total = 0, n;
                        while ((n = stream.read(buffer)) != -1) {
                            total += n;
                            if (total > limit) throw new IOException("Document exceeds the import size limit");
                            local.write(buffer, 0, n);
                        }
                    }
                    result = "file\n" + temporary.getAbsolutePath() + "\n" + name;
                    temporary = null; // Rust consumes and removes its private cache copy.
                }
            } catch (Exception error) {
                result = "error\n" + error.getMessage();
            } finally {
                if (temporary != null) temporary.delete();
            }
            }
        }, "Leviathan Files").start();
    }

    private String displayName(Uri uri, String fallback) {
        try (Cursor cursor = getContentResolver().query(uri, new String[]{OpenableColumns.DISPLAY_NAME},
                                                        null, null, null)) {
            if (cursor != null && cursor.moveToFirst() && !cursor.isNull(0) && !cursor.getString(0).isEmpty()) {
                return cursor.getString(0);
            }
        } catch (RuntimeException ignored) { /* Filename metadata is optional. */ }
        return fallback;
    }
}
