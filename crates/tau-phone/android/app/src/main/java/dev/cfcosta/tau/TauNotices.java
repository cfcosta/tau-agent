package dev.cfcosta.tau;

import android.Manifest;
import android.app.Activity;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;

/** Notifications from tau's agents (ADR 0037): a note or a question one
 *  left, or one that spent its day. The phone hears of them while it is
 *  connected to the computer; there is no push for when it is not.
 *  Called from native threads, never the UI's. */
public final class TauNotices {
    private TauNotices() {}

    /** The channel agents' notices go to, which the person can mute in
     *  Android's settings. */
    private static final String CHANNEL = "agents";

    /** Asked once a run of the app, the first time a notice comes. */
    private static volatile boolean asked;

    /** The extra a notice's tap carries: the agent it is about. */
    private static final String AGENT = "dev.cfcosta.tau.agent";

    /** The agent whose notice the person tapped, until tau takes it. */
    private static volatile String opened;

    /** Notes the agent `intent` was opened for, if a notice's tap
     *  opened it. The activity calls it as it starts and as it is
     *  brought back. */
    public static void heard(Intent intent) {
        String agent = intent == null ? null : intent.getStringExtra(AGENT);
        if (agent != null) opened = agent;
    }

    /** The agent whose notice was tapped since tau last asked, or null;
     *  asking clears it. */
    public static String takeOpened() {
        String agent = opened;
        opened = null;
        return agent;
    }

    /** Shows `title` and `body` as a notification; `tag` keeps one per
     *  notice, so the same one shown again replaces itself. Tapping it
     *  opens tau on `agent`'s conversation. Without the person's leave
     *  to notify, it asks for it once and shows nothing this time. */
    public static void show(
            Activity activity, String tag, String agent, String title, String body) {
        if (activity.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS)
                != PackageManager.PERMISSION_GRANTED) {
            if (!asked) {
                asked = true;
                activity.runOnUiThread(() -> activity.requestPermissions(
                        new String[] {Manifest.permission.POST_NOTIFICATIONS}, 7));
            }
            return;
        }
        NotificationManager manager =
                (NotificationManager) activity.getSystemService(Context.NOTIFICATION_SERVICE);
        if (manager == null) return;
        manager.createNotificationChannel(new NotificationChannel(
                CHANNEL, "Agents", NotificationManager.IMPORTANCE_DEFAULT));
        Intent open = new Intent(activity, activity.getClass())
                .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
                .putExtra(AGENT, agent);
        // Each notice its own request, so its tap keeps its own agent.
        PendingIntent tap = PendingIntent.getActivity(
                activity, tag.hashCode(), open,
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        Notification notice = new Notification.Builder(activity, CHANNEL)
                .setSmallIcon(android.R.drawable.stat_notify_chat)
                .setContentTitle(title)
                .setContentText(body)
                .setStyle(new Notification.BigTextStyle().bigText(body))
                .setContentIntent(tap)
                .setAutoCancel(true)
                .build();
        manager.notify(tag, 0, notice);
    }
}
