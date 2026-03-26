import { useEffect, useCallback } from "react";
import { useSettingsStore } from "../store/settings";
import type { DndSchedule, NotificationOverride, NotificationLevel } from "../store/settings";

async function requestOsPermission(): Promise<void> {
  try {
    const { isPermissionGranted, requestPermission, createChannel, Importance } = await import(
      "@tauri-apps/plugin-notification"
    );
    if (!(await isPermissionGranted())) {
      await requestPermission();
    }
    // Create a default channel for Android
    await createChannel({
      id: "messages",
      name: "Messages",
      description: "Notifications for new messages",
      importance: Importance.High,
      vibration: true,
      sound: "default",
    });
  } catch {
    // Not in a Tauri environment or platform doesn't support channels
  }
}

function playNotificationSound(): void {
  try {
    const ctx = new AudioContext();
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();
    osc.connect(gain);
    gain.connect(ctx.destination);
    osc.type = "sine";
    osc.frequency.setValueAtTime(880, ctx.currentTime);
    osc.frequency.exponentialRampToValueAtTime(440, ctx.currentTime + 0.12);
    gain.gain.setValueAtTime(0.25, ctx.currentTime);
    gain.gain.exponentialRampToValueAtTime(0.001, ctx.currentTime + 0.35);
    osc.start(ctx.currentTime);
    osc.stop(ctx.currentTime + 0.35);
    osc.onended = () => { void ctx.close(); };
  } catch {
    // Audio not available in this environment
  }
}

async function dispatchOsNotification(title: string, body: string): Promise<void> {
  try {
    const { isPermissionGranted, sendNotification } = await import(
      "@tauri-apps/plugin-notification"
    );
    if (await isPermissionGranted()) {
      sendNotification({
        title,
        body,
        channelId: "messages", // Required for Android
      });
    }
  } catch {
    // Not in a Tauri environment
  }
}

export function isInDnd(schedule: DndSchedule): boolean {
  if (!schedule.enabled) return false;
  const hour = new Date().getHours();
  const { startHour, endHour } = schedule;
  // Handle overnight schedules (e.g. 22:00–08:00)
  return startHour > endHour
    ? hour >= startHour || hour < endHour
    : hour >= startHour && hour < endHour;
}

function resolveNotificationLevel(
  overrides: NotificationOverride[],
  defaultLevel: NotificationLevel,
  communityId: string,
  channelId: string,
): NotificationLevel {
  const communityOverride = overrides.find((o) => o.communityId === communityId);
  const channelLevel = communityOverride?.channelOverrides[channelId];
  return channelLevel ?? communityOverride?.level ?? defaultLevel;
}

/**
 * Provides OS notification helpers that respect the user's notification settings
 * (enabled toggle, DND schedule, notification level, per-channel overrides).
 */
export function useOsNotifications() {
  const osNotificationsEnabled = useSettingsStore((s) => s.osNotificationsEnabled);

  // Request permission once when OS notifications are enabled.
  useEffect(() => {
    if (osNotificationsEnabled) void requestOsPermission();
  }, [osNotificationsEnabled]);

  /**
   * Show a notification for a community channel message.
   * Skips if the notification level for this channel is not "all".
   */
  const notifyMessage = useCallback(
    (communityId: string, channelId: string, title: string, body: string) => {
      const {
        osNotificationsEnabled,
        soundEnabled,
        dndSchedule,
        defaultNotificationLevel,
        notificationOverrides,
      } = useSettingsStore.getState();
      if (isInDnd(dndSchedule)) return;
      const level = resolveNotificationLevel(
        notificationOverrides,
        defaultNotificationLevel,
        communityId,
        channelId,
      );
      if (level !== "all") return;
      if (soundEnabled) playNotificationSound();
      if (osNotificationsEnabled) void dispatchOsNotification(title, body);
    },
    [],
  );

  /**
   * Show a notification for an incoming direct message.
   * Skips only if the global notification level is "none".
   */
  const notifyDm = useCallback((title: string, body: string) => {
    const { osNotificationsEnabled, soundEnabled, dndSchedule, defaultNotificationLevel } =
      useSettingsStore.getState();
    if (isInDnd(dndSchedule)) return;
    if (defaultNotificationLevel === "none") return;
    if (soundEnabled) playNotificationSound();
    if (osNotificationsEnabled) void dispatchOsNotification(title, body);
  }, []);

  return { notifyMessage, notifyDm };
}
