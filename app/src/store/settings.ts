import { create } from "zustand";
import { persist } from "zustand/middleware";

export type Theme = "dark" | "light" | "system";
export type FontSize = "small" | "medium" | "large";
export type MessageDensity = "compact" | "cozy" | "comfortable";
export type NotificationLevel = "all" | "mentions" | "none";

export interface NotificationOverride {
  communityId: string;
  level: NotificationLevel;
  channelOverrides: Record<string, NotificationLevel>;
}

export interface DndSchedule {
  enabled: boolean;
  startHour: number; // 0-23
  endHour: number;   // 0-23
}

export interface AppSettings {
  // Appearance
  theme: Theme;
  fontSize: FontSize;
  messageDensity: MessageDensity;
  animatedEmoji: boolean;

  // Notifications
  defaultNotificationLevel: NotificationLevel;
  notificationOverrides: NotificationOverride[];
  osNotificationsEnabled: boolean;
  soundEnabled: boolean;
  dndSchedule: DndSchedule;
}

interface SettingsState extends AppSettings {
  setTheme: (theme: Theme) => void;
  setFontSize: (size: FontSize) => void;
  setMessageDensity: (density: MessageDensity) => void;
  setAnimatedEmoji: (enabled: boolean) => void;
  setDefaultNotificationLevel: (level: NotificationLevel) => void;
  setNotificationOverride: (communityId: string, level: NotificationLevel) => void;
  setChannelNotificationOverride: (communityId: string, channelId: string, level: NotificationLevel) => void;
  setOsNotificationsEnabled: (enabled: boolean) => void;
  setSoundEnabled: (enabled: boolean) => void;
  setDndSchedule: (schedule: DndSchedule) => void;
  resetToDefaults: () => void;
}

const DEFAULT_SETTINGS: AppSettings = {
  theme: "dark",
  fontSize: "medium",
  messageDensity: "cozy",
  animatedEmoji: true,
  defaultNotificationLevel: "all",
  notificationOverrides: [],
  osNotificationsEnabled: true,
  soundEnabled: true,
  dndSchedule: { enabled: false, startHour: 22, endHour: 8 },
};

export const useSettingsStore = create<SettingsState>()(
  persist(
    (set) => ({
      ...DEFAULT_SETTINGS,

      setTheme: (theme) => set({ theme }),
      setFontSize: (fontSize) => set({ fontSize }),
      setMessageDensity: (messageDensity) => set({ messageDensity }),
      setAnimatedEmoji: (animatedEmoji) => set({ animatedEmoji }),
      setDefaultNotificationLevel: (defaultNotificationLevel) => set({ defaultNotificationLevel }),

      setNotificationOverride: (communityId, level) =>
        set((s) => {
          const overrides = s.notificationOverrides.filter((o) => o.communityId !== communityId);
          overrides.push({ communityId, level, channelOverrides: {} });
          return { notificationOverrides: overrides };
        }),

      setChannelNotificationOverride: (communityId, channelId, level) =>
        set((s) => {
          const overrides = s.notificationOverrides.map((o) => {
            if (o.communityId !== communityId) return o;
            return { ...o, channelOverrides: { ...o.channelOverrides, [channelId]: level } };
          });
          if (!overrides.find((o) => o.communityId === communityId)) {
            overrides.push({ communityId, level: s.defaultNotificationLevel, channelOverrides: { [channelId]: level } });
          }
          return { notificationOverrides: overrides };
        }),

      setOsNotificationsEnabled: (osNotificationsEnabled) => set({ osNotificationsEnabled }),
      setSoundEnabled: (soundEnabled) => set({ soundEnabled }),
      setDndSchedule: (dndSchedule) => set({ dndSchedule }),
      resetToDefaults: () => set({ ...DEFAULT_SETTINGS }),
    }),
    { name: "bitcord-settings" }
  )
);
