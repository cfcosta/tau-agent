package dev.cfcosta.tau;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.database.Cursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;

import java.io.File;
import java.io.FileNotFoundException;

/** Lets the camera app write one photo into tau's cache, and nothing
 *  else: FileProvider's job, without AndroidX. */
public class TauPhotos extends ContentProvider {
    static final String AUTHORITY = "dev.cfcosta.tau.photos";
    static final String DIR = "scans";

    static Uri uri(File photo) {
        return new Uri.Builder()
                .scheme("content")
                .authority(AUTHORITY)
                .appendPath(photo.getName())
                .build();
    }

    @Override public boolean onCreate() {
        return true;
    }

    @Override public ParcelFileDescriptor openFile(Uri uri, String mode)
            throws FileNotFoundException {
        String name = uri.getLastPathSegment();
        if (name == null || name.contains("/") || name.startsWith(".")) {
            throw new FileNotFoundException(String.valueOf(uri));
        }
        File dir = new File(getContext().getCacheDir(), DIR);
        dir.mkdirs();
        return ParcelFileDescriptor.open(
                new File(dir, name), ParcelFileDescriptor.parseMode(mode));
    }

    @Override public String getType(Uri uri) {
        return "image/jpeg";
    }

    @Override public Cursor query(Uri uri, String[] projection, String selection,
            String[] args, String order) {
        return null;
    }

    @Override public Uri insert(Uri uri, ContentValues values) {
        return null;
    }

    @Override public int delete(Uri uri, String selection, String[] args) {
        return 0;
    }

    @Override public int update(Uri uri, ContentValues values, String selection,
            String[] args) {
        return 0;
    }
}
