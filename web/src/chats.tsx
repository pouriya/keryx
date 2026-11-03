import { Check, CheckCheck, Menu } from "lucide-react";
import { useEffect, useState } from "react";
import { Virtuoso } from "react-virtuoso";
import styles from "./chats.module.css";
import { Problem } from "./problem";
import { type ChatRow, submit, useStore } from "./store";
import { useText } from "./text";

function letter(name: string) {
  return [...name][0]?.toUpperCase() ?? "";
}

function when(at: number | null, language: string) {
  if (at === null) {
    return "";
  }
  const date = new Date(at);
  const today = new Date().toDateString() === date.toDateString();
  return new Intl.DateTimeFormat(
    language,
    today ? { hour: "numeric", minute: "2-digit" } : { month: "short", day: "numeric" },
  ).format(date);
}

function ticks(row: ChatRow, me: string | null) {
  if (row.type !== 1 || row.previewAccount === null || row.previewAccount !== me) {
    return "none" as const;
  }
  const seq = row.lastContentSeq;
  const other = row.marks[0];
  if (seq === null || !other || other.receivedSeq < seq) {
    return "one" as const;
  }
  if (other.seenSeq !== null && other.seenSeq >= seq) {
    return "seen" as const;
  }
  return "two" as const;
}

export function Chats() {
  const rows = useStore((state) => state.rows);
  const people = useStore((state) => state.people);
  const me = useStore((state) => state.me);
  const language = useStore((state) => state.language);
  const busy = useStore((state) => state.busy);
  const t = useText();
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!open) {
      return;
    }
    const close = () => setOpen(false);
    window.addEventListener("pointerdown", close);
    return () => window.removeEventListener("pointerdown", close);
  }, [open]);

  return (
    <div className={styles.shell}>
      <section className={styles.column}>
        <header className={styles.header}>
          <button
            className={styles.icon}
            type="button"
            aria-label={t("menu")}
            aria-expanded={open}
            onPointerDown={(event) => event.stopPropagation()}
            onClick={() => setOpen((value) => !value)}
          >
            <Menu />
          </button>
          {open ? (
            <div className={styles.menu} onPointerDown={(event) => event.stopPropagation()}>
              <button type="button">{t("devices")}</button>
              <button type="button" disabled={busy} onClick={() => submit({ type: "log_out" })}>
                {t("logOut")}
              </button>
            </div>
          ) : null}
        </header>
        <Problem />
        <Virtuoso
          className={styles.list}
          data={rows}
          computeItemKey={(_, row) => row.id}
          itemContent={(_, row) => {
            const person = row.marks.map((mark) => people[mark.account]).find((item) => item);
            const name =
              row.type === 0 ? t("savedMessages") : person?.displayName || person?.username || "";
            const mark = ticks(row, me);
            return (
              <div className={styles.row}>
                <div className={styles.letter}>{letter(name)}</div>
                <div className={styles.body}>
                  <div className={styles.nameLine}>
                    <span className={styles.name}>{name}</span>
                    <span className={styles.time}>{when(row.lastAt, language)}</span>
                  </div>
                  <div className={styles.previewLine}>
                    <span className={styles.preview}>
                      {mark === "one" || mark === "two" || mark === "seen" ? (
                        <span className={mark === "seen" ? styles.seen : styles.ticks}>
                          {mark === "one" ? <Check size={16} /> : <CheckCheck size={16} />}
                        </span>
                      ) : null}
                      <span className={styles.previewText}>
                        {row.lastContentSeq === null ? t("noMessages") : row.preview}
                      </span>
                    </span>
                    {row.unread > 0 ? <span className={styles.unread}>{row.unread}</span> : null}
                  </div>
                </div>
              </div>
            );
          }}
        />
      </section>
      <main className={styles.rest}>
        <p>{t("next")}</p>
      </main>
    </div>
  );
}
