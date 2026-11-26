package io.github.josiahalanlee.leviathan;

import android.app.Activity;
import android.app.NativeActivity;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.provider.DocumentsContract;
import android.provider.OpenableColumns;
import java.util.Arrays;
import java.util.Comparator;
import java.io.File;
import java.io.FileFilter;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;

/** The Rust engine stays in NativeActivity; only the document result crosses here. */
public final class DocumentActivity extends NativeActivity {
    private static final int WAV = 1, OPEN = 2, SAVE = 3, EXPORT_WAV = 4, EXPORT_FOLDER = 5;
    // Called only on the UI thread, and shared across Activity instances in this process.
    private static int nextRequest = 4700;
    private static final int WAV_LIMIT = 64 * 1024 * 1024, PROJECT_LIMIT = 8 * 1024 * 1024;
    private int operation;
    private int requestCode;
    private String sourcePath;
    private volatile String result;

    public void launchDocument(int kind, String path) {
        if (kind < WAV || kind > EXPORT_FOLDER || operation != 0) return;
        result = null;
        operation = kind;
        sourcePath = path;
        requestCode = nextRequest;
        nextRequest = nextRequest >= 0x7fff ? 4700 : nextRequest + 1;
        String action = kind == EXPORT_FOLDER ? Intent.ACTION_OPEN_DOCUMENT_TREE :
            kind == SAVE || kind == EXPORT_WAV ? Intent.ACTION_CREATE_DOCUMENT : Intent.ACTION_OPEN_DOCUMENT;
        Intent intent = new Intent(action);
        if (kind != EXPORT_FOLDER) {
            intent.addCategory(Intent.CATEGORY_OPENABLE);
            if (kind == SAVE || kind == EXPORT_WAV) {
                intent.setType(kind == SAVE ? "application/json" : "audio/wav");
                intent.putExtra(Intent.EXTRA_TITLE, kind == SAVE ? "untitled.leviathan.json" : "leviathan.wav");
            } else {
                // Several providers advertise WAV and JSON as octet-stream. Rust validates bytes.
                intent.setType("*/*");
            }
        }
        try {
            startActivityForResult(intent, requestCode);
        } catch (RuntimeException error) {
            sourcePath = null;
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
        // A recreated NativeActivity deliberately cancels rather than replaying a write.
        // Rust exposes the private pre-picker project for recovery.
        if (operation == 0 || code != requestCode) return;
        final int kind = operation;
        final String source = sourcePath;
        sourcePath = null;
        final Uri uri = intent == null ? null : intent.getData();
        if (status != Activity.RESULT_OK || uri == null) {
            result = "cancel";
            return;
        }

        new Thread(new Runnable() {
            @Override public void run() {
            File temporary = null;
            try {
                if (kind == SAVE || kind == EXPORT_WAV || kind == EXPORT_FOLDER) {
                    File files = getFilesDir().getCanonicalFile();
                    File staged = new File(source).getCanonicalFile();
                    if (kind == SAVE) {
                        if (!staged.equals(new File(files, "files-recovery.leviathan.json"))
                            || !staged.isFile() || staged.length() > PROJECT_LIMIT) {
                            throw new IOException("Invalid staged project");
                        }
                        writeSelectedFile(staged, uri);
                    } else {
                        File root = new File(files, "wav-staging");
                        if (!root.equals(staged.getParentFile()) || !staged.isDirectory()) {
                            throw new IOException("Invalid WAV staging folder");
                        }
                        File output = new File(staged, "mix.wav");
                        if (kind == EXPORT_WAV) {
                            if (!output.isFile()) throw new IOException("Master WAV is missing");
                            writeSelectedFile(output, uri);
                        } else {
                            writeBundle(output, uri);
                        }
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

    private void copyToDocument(File source, Uri destination) throws IOException {
        try (InputStream input = new FileInputStream(source);
             OutputStream output = getContentResolver().openOutputStream(destination, "w")) {
            if (output == null) throw new IOException("Files did not provide a writable document");
            byte[] buffer = new byte[65536];
            int n;
            while ((n = input.read(buffer)) != -1) output.write(buffer, 0, n);
        }
    }

    private boolean deleteCreated(Uri document) {
        try {
            return DocumentsContract.deleteDocument(getContentResolver(), document);
        } catch (Exception ignored) {
            return false;
        }
    }

    private void writeSelectedFile(File source, Uri destination) throws IOException {
        try {
            copyToDocument(source, destination);
        } catch (Exception error) {
            boolean removed = deleteCreated(destination);
            throw new IOException("Could not write the selected file."
                + (removed ? "" : " Check Files for an incomplete copy."), error);
        }
    }

    private void writeBundle(File source, Uri tree) throws IOException {
        File[] outputs = source.listFiles(new FileFilter() {
            @Override public boolean accept(File file) {
                return file.isFile() && file.getName().endsWith(".wav");
            }
        });
        if (outputs == null || outputs.length == 0 || !new File(source, "master.wav").isFile()) {
            throw new IOException("No rendered WAV bundle is available");
        }
        Arrays.sort(outputs, new Comparator<File>() {
            @Override public int compare(File left, File right) {
                return left.getName().compareTo(right.getName());
            }
        });
        Uri parent = DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree));
        Uri folder = DocumentsContract.createDocument(getContentResolver(), parent,
            DocumentsContract.Document.MIME_TYPE_DIR, "Leviathan export");
        if (folder == null) throw new IOException("Files could not create a new export folder");
        try {
            for (File file : outputs) {
                Uri treeFolder = DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getDocumentId(folder));
                Uri child = DocumentsContract.createDocument(getContentResolver(), treeFolder, "audio/wav", file.getName());
                if (child == null) throw new IOException("Files could not create " + file.getName());
                copyToDocument(file, DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getDocumentId(child)));
            }
        } catch (Exception error) {
            boolean removed = deleteCreated(DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getDocumentId(folder)));
            throw new IOException("Could not finish the WAV bundle."
                + (removed ? "" : " Check Files for an incomplete Leviathan export folder."), error);
        }
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
