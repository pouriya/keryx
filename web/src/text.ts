import en from "./en.json" with { type: "json" };
import fa from "./fa.json" with { type: "json" };
import { type Language, useStore } from "./store";

const faMessages: typeof en = fa;
const MESSAGES: Record<Language, typeof en> = { en, fa: faMessages };

export type Key = keyof typeof en;

export function useText() {
  const language = useStore((state) => state.language);
  return (key: Key, values?: Record<string, string | number>) => {
    let value: string = MESSAGES[language][key];
    if (values) {
      for (const [name, raw] of Object.entries(values)) {
        const shown = typeof raw === "number" ? new Intl.NumberFormat(language).format(raw) : raw;
        value = value.replaceAll(`{${name}}`, shown);
      }
    }
    return value;
  };
}
