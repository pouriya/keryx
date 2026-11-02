import { useEffect, useMemo, useRef, useState } from "react";
import { Problem } from "./problem";
import screen from "./session.module.css";
import { submit, useStore } from "./store";
import { useText } from "./text";
import { isPrefix, isWord, WORDS } from "./wordlist";
import styles from "./words.module.css";

function pickThree(length: number): [number, number, number] {
  const bag = Array.from({ length }, (_, index) => index);
  const random = new Uint32Array(bag.length);
  crypto.getRandomValues(random);
  for (let index = bag.length - 1; index > 0; index--) {
    const roll = random[index];
    const at = bag[index];
    const swapAt = roll === undefined ? 0 : roll % (index + 1);
    const swap = bag[swapAt];
    if (at === undefined || swap === undefined) {
      continue;
    }
    bag[index] = swap;
    bag[swapAt] = at;
  }
  const chosen = bag.slice(0, 3).sort((left, right) => left - right);
  const first = chosen[0];
  const second = chosen[1];
  const third = chosen[2];
  if (first === undefined || second === undefined || third === undefined) {
    return [0, 1, 2];
  }
  return [first, second, third];
}

export function Words() {
  const restore = useStore((state) => state.restore);
  return restore ? <Restore /> : <NewWords />;
}

function Why() {
  const t = useText();
  const [open, setOpen] = useState(false);
  const button = useRef<HTMLButtonElement>(null);
  const close = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!open) {
      return;
    }
    close.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setOpen(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open]);
  return (
    <>
      <button
        ref={button}
        type="button"
        className={styles.info}
        aria-expanded={open}
        aria-label={t("wordsWhyTitle")}
        onClick={() => setOpen(true)}
      >
        !
      </button>
      {open ? (
        <div className={styles.shade}>
          <div className={styles.card} role="dialog" aria-modal="true" aria-labelledby="words-why">
            <h2 id="words-why">{t("wordsWhyTitle")}</h2>
            <p>{t("wordsWhy", { name: t("name") })}</p>
            <button
              ref={close}
              type="button"
              className={screen.primary}
              onClick={() => {
                setOpen(false);
                button.current?.focus();
              }}
            >
              {t("wordsGotIt")}
            </button>
          </div>
          <button
            type="button"
            className={styles.backdrop}
            aria-label={t("wordsGotIt")}
            onClick={() => {
              setOpen(false);
              button.current?.focus();
            }}
          />
        </div>
      ) : null}
    </>
  );
}

function Length({ count, onPick }: { count: 12 | 24; onPick: (count: 12 | 24) => void }) {
  const t = useText();
  return (
    <div className={styles.choice}>
      {([12, 24] as const).map((value) => (
        <button
          key={value}
          type="button"
          aria-pressed={count === value}
          onClick={() => onPick(value)}
        >
          {t("length", { n: value })}
        </button>
      ))}
    </div>
  );
}

function WordList() {
  return (
    <datalist id="bip39">
      {WORDS.map((word) => (
        <option key={word} value={word} />
      ))}
    </datalist>
  );
}

function NewWords() {
  const t = useText();
  const words = useStore((state) => state.words);
  const send = useStore((state) => state.send);
  const busy = useStore((state) => state.busy);
  const forgetWords = useStore((state) => state.forgetWords);
  const [count, setCount] = useState<12 | 24>(12);
  const [written, setWritten] = useState(false);
  const [answers, setAnswers] = useState(["", "", ""]);
  useEffect(() => {
    send({ type: "generate_words", count });
  }, [count, send]);
  useEffect(() => {
    if (words === null || words.length > 0) {
      setWritten(false);
      setAnswers(["", "", ""]);
    }
  }, [words]);
  const parts = words?.split(" ") ?? [];
  const places = useMemo(() => {
    const count = words === null ? 0 : words.split(" ").length;
    return count >= 3 ? pickThree(count) : null;
  }, [words]);
  const ready =
    places?.every((place, index) => answers[index]?.trim().toLowerCase() === parts[place]) ?? false;
  const filled = places?.every((_, index) => (answers[index] ?? "").trim() !== "") ?? false;
  return (
    <main className={`${screen.screen} ${styles.page}`}>
      <div className={styles.heading}>
        <h1>{t("wordsTitle")}</h1>
        <Why />
      </div>
      <Length
        count={count}
        onPick={(next) => {
          if (next !== count) {
            setCount(next);
            forgetWords();
          }
        }}
      />
      {words !== null && places !== null && !written ? (
        <>
          <p className={styles.lead}>{t("wordsLead")}</p>
          <ol className={styles.phrase} lang="en" dir="ltr">
            {parts
              .map((word, position) => ({ word, position }))
              .map((line) => (
                <li key={line.position}>
                  <span className={styles.index}>{line.position + 1}</span>
                  <span className={styles.token}>{line.word}</span>
                </li>
              ))}
          </ol>
          <div className={`${screen.actions} ${styles.actions}`}>
            <button className={screen.primary} type="button" onClick={() => setWritten(true)}>
              {t("wroteThem")}
            </button>
          </div>
        </>
      ) : null}
      {words !== null && places !== null && written ? (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (ready) {
              submit({ type: "set_words", words });
            }
          }}
        >
          {places.map((place, index) => (
            <div key={place} className={screen.field} lang="en" dir="ltr">
              <input
                id={`word-${place}`}
                value={answers[index] ?? ""}
                placeholder=" "
                autoComplete="off"
                autoCapitalize="off"
                autoCorrect="off"
                spellCheck={false}
                list="bip39"
                onChange={(event) => {
                  const next = answers.slice();
                  next[index] = event.target.value;
                  setAnswers(next);
                }}
              />
              <label htmlFor={`word-${place}`}>{t("word", { n: place + 1 })}</label>
            </div>
          ))}
          <WordList />
          <Problem />
          {filled && !ready ? <p className={styles.wrong}>{t("wordsMismatch")}</p> : null}
          <div className={`${screen.actions} ${styles.actions}`}>
            <button className={screen.primary} type="submit" disabled={!ready || busy}>
              {t("continue")}
            </button>
            <button className={screen.quiet} type="button" onClick={() => setWritten(false)}>
              {t("back")}
            </button>
          </div>
        </form>
      ) : null}
    </main>
  );
}

function Restore() {
  const t = useText();
  const [count, setCount] = useState<12 | 24>(12);
  const [boxes, setBoxes] = useState<string[]>(() => Array<string>(12).fill(""));
  const sent = useRef("");
  const clearError = useStore((state) => state.clearError);
  useEffect(() => {
    if (boxes.some((box) => !isWord(box))) {
      return;
    }
    const phrase = boxes.join(" ");
    if (phrase === sent.current) {
      return;
    }
    sent.current = phrase;
    submit({ type: "restore", words: phrase });
  }, [boxes]);
  return (
    <main className={`${screen.screen} ${styles.page}`}>
      <div className={styles.heading}>
        <h1>{t("wordsTitle")}</h1>
        <Why />
      </div>
      <p className={styles.lead}>{t("restoreLead")}</p>
      <Length
        count={count}
        onPick={(next) => {
          if (next === count) {
            return;
          }
          setCount(next);
          setBoxes((current) => Array.from({ length: next }, (_, index) => current[index] ?? ""));
          sent.current = "";
          clearError();
        }}
      />
      {boxes.some((value) => value !== "" && !isPrefix(value)) ? (
        <p className={styles.wrong}>{t("wrongWord")}</p>
      ) : null}
      <div className={styles.boxes}>
        {boxes
          .map((value, position) => ({ value, position }))
          .map((field) => {
            const bad = field.value !== "" && !isPrefix(field.value);
            return (
              <label key={field.position} lang="en" dir="ltr">
                <span>{field.position + 1}</span>
                <input
                  value={field.value}
                  aria-label={t("word", { n: field.position + 1 })}
                  aria-invalid={bad}
                  autoComplete="off"
                  autoCapitalize="off"
                  autoCorrect="off"
                  spellCheck={false}
                  list="bip39"
                  onChange={(event) => {
                    const next = boxes.slice();
                    next[field.position] = event.target.value.trim().toLowerCase();
                    setBoxes(next);
                    clearError();
                  }}
                  onPaste={(event) => {
                    const pasted = event.clipboardData
                      .getData("text")
                      .trim()
                      .toLowerCase()
                      .split(/\s+/)
                      .filter((part) => part !== "");
                    if (pasted.length < 2) {
                      return;
                    }
                    event.preventDefault();
                    setBoxes((current) => {
                      const next = current.slice();
                      for (let offset = 0; offset < pasted.length; offset++) {
                        const word = pasted[offset];
                        const at = field.position + offset;
                        if (word !== undefined && at < next.length) {
                          next[at] = word;
                        }
                      }
                      return next;
                    });
                    sent.current = "";
                    clearError();
                  }}
                />
              </label>
            );
          })}
      </div>
      <WordList />
      <Problem />
    </main>
  );
}
