import { useEffect } from "react";
import { useSettingsStore } from "../store/settings";

const FONT_SIZE_MAP = {
  small: "13px",
  medium: "15px",
  large: "17px",
};

function applyTheme(resolved: "dark" | "light") {
  if (resolved === "light") {
    document.documentElement.dataset.theme = "light";
  } else {
    delete document.documentElement.dataset.theme;
  }
}

function resolveSystemTheme(): "dark" | "light" {
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export function useTheme() {
  const theme = useSettingsStore((s) => s.theme);
  const fontSize = useSettingsStore((s) => s.fontSize);

  useEffect(() => {
    if (theme !== "system") {
      applyTheme(theme);
      return;
    }

    applyTheme(resolveSystemTheme());
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const handler = (e: MediaQueryListEvent) => applyTheme(e.matches ? "dark" : "light");
    mq.addEventListener("change", handler);
    return () => mq.removeEventListener("change", handler);
  }, [theme]);

  useEffect(() => {
    document.documentElement.style.fontSize = FONT_SIZE_MAP[fontSize];
  }, [fontSize]);
}
