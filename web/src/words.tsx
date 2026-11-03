import { useEffect, useMemo, useRef, useState } from "react";
import { Problem } from "./problem";
import screen from "./session.module.css";
import { submit, useStore } from "./store";
import { useText } from "./text";
import { isPrefix, isWord, matching } from "./wordlist";
import styles from "./words.module.css";

const MATCHES = 5;

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
  const [fresh, setFresh] = useState(false);
  if (!restore || fresh) {
    return <NewWords />;
  }
  return <Restore onLost={() => setFresh(true)} />;
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

function Suggest({ prefix, onPick }: { prefix: string; onPick: (word: string) => void }) {
  const found = matching(prefix.trim().toLowerCase(), MATCHES);
  if (found.length === 0) {
    return null;
  }
  return (
    <ul className={styles.matches} lang="en" dir="ltr">
      {found.map((word) => (
        <li key={word}>
          <button
            type="button"
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => onPick(word)}
          >
            {word}
          </button>
        </li>
      ))}
    </ul>
  );
}

function WordField({
  id,
  label,
  value,
  active,
  onFocus,
  onBlur,
  onChange,
  onPaste,
}: {
  id: string;
  label: string;
  value: string;
  active: boolean;
  onFocus: () => void;
  onBlur: () => void;
  onChange: (value: string) => void;
  onPaste?: (words: string[]) => void;
}) {
  return (
    <div className={screen.field} lang="en" dir="ltr">
      <input
        id={id}
        value={value}
        placeholder=" "
        autoComplete="off"
        autoCapitalize="off"
        autoCorrect="off"
        spellCheck={false}
        aria-invalid={value !== "" && !isPrefix(value)}
        onFocus={onFocus}
        onBlur={onBlur}
        onChange={(event) => onChange(event.target.value.trim().toLowerCase())}
        onPaste={(event) => {
          if (onPaste === undefined) {
            return;
          }
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
          onPaste(pasted);
        }}
      />
      <label htmlFor={id}>{label}</label>
      {active ? <Suggest prefix={value} onPick={onChange} /> : null}
    </div>
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
  const [focus, setFocus] = useState<number | null>(null);
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
    const length = words === null ? 0 : words.split(" ").length;
    return length >= 3 ? pickThree(length) : null;
  }, [words]);
  const ready =
    places?.every((place, index) => answers[index]?.trim().toLowerCase() === parts[place]) ?? false;
  const filled = places?.every((_, index) => (answers[index] ?? "").trim() !== "") ?? false;
  const setAnswer = (index: number, value: string) => {
    const next = answers.slice();
    next[index] = value;
    setAnswers(next);
  };
  if (words === null || places === null) {
    return (
      <main className={`${screen.screen} ${styles.page}`}>
        <h1>{t("wordsTitle")}</h1>
      </main>
    );
  }
  return (
    <main className={`${screen.screen} ${styles.page}`}>
      {!written ? (
        <>
          <div className={styles.top}>
            <h1>{t("wordsTitle")}</h1>
            <p className={styles.lead}>{t("wordsLead")}</p>
            <Length
              count={count}
              onPick={(next) => {
                if (next !== count) {
                  setCount(next);
                  forgetWords();
                }
              }}
            />
          </div>
          <div className={styles.middle}>
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
          </div>
          <div className={`${screen.actions} ${styles.bottom}`}>
            <button className={screen.primary} type="button" onClick={() => setWritten(true)}>
              {t("wroteThem")}
            </button>
          </div>
        </>
      ) : null}
      {written ? (
        <form
          className={styles.stack}
          onSubmit={(event) => {
            event.preventDefault();
            if (ready) {
              submit({ type: "set_words", words });
            }
          }}
        >
          <div className={styles.top}>
            <h1>{t("wordsTitle")}</h1>
            <p className={styles.lead}>
              {t("checkLead", {
                a: (places[0] ?? 0) + 1,
                b: (places[1] ?? 0) + 1,
                c: (places[2] ?? 0) + 1,
              })}
            </p>
          </div>
          <div className={styles.middle}>
            <div className={styles.entry}>
              {places.map((place, index) => (
                <WordField
                  key={place}
                  id={`word-${place}`}
                  label={t("word", { n: place + 1 })}
                  value={answers[index] ?? ""}
                  active={focus === index}
                  onFocus={() => setFocus(index)}
                  onBlur={() => setFocus(null)}
                  onChange={(value) => setAnswer(index, value)}
                />
              ))}
            </div>
          </div>
          <Problem />
          {filled && !ready ? <p className={styles.wrong}>{t("wordsMismatch")}</p> : null}
          <div className={`${screen.actions} ${styles.bottom}`}>
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

function Restore({ onLost }: { onLost: () => void }) {
  const t = useText();
  const [count, setCount] = useState<12 | 24>(12);
  const [boxes, setBoxes] = useState<string[] | null>(null);
  const [lost, setLost] = useState(false);
  const [index, setIndex] = useState(0);
  const [active, setActive] = useState(false);
  const sent = useRef("");
  const clearError = useStore((state) => state.clearError);
  const busy = useStore((state) => state.busy);
  const current = boxes?.[index] ?? "";
  const submitPhrase = (phrase: string[]) => {
    if (phrase.some((word) => !isWord(word))) {
      return;
    }
    const text = phrase.join(" ");
    if (text === sent.current) {
      return;
    }
    sent.current = text;
    submit({ type: "restore", words: text });
  };
  const edit = (at: number, value: string) => {
    setBoxes((existing) => {
      if (existing === null) {
        return existing;
      }
      const next = existing.slice();
      next[at] = value;
      return next;
    });
    sent.current = "";
    clearError();
  };
  const spread = (pasted: string[]) => {
    if (boxes === null) {
      return;
    }
    const next = boxes.slice();
    for (let offset = 0; offset < pasted.length; offset++) {
      const word = pasted[offset];
      const at = index + offset;
      if (word !== undefined && at < next.length) {
        next[at] = word;
      }
    }
    setBoxes(next);
    sent.current = "";
    clearError();
    if (next.every((word) => isWord(word))) {
      submitPhrase(next);
    }
  };
  if (boxes === null && lost) {
    return (
      <main className={`${screen.screen} ${styles.page}`}>
        <div className={styles.top}>
          <h1>{t("lostTitle")}</h1>
        </div>
        <div className={styles.middle}>
          <p className={styles.lead}>{t("lostBody")}</p>
        </div>
        <div className={`${screen.actions} ${styles.bottom}`}>
          <button className={screen.primary} type="button" onClick={onLost}>
            {t("lostContinue")}
          </button>
          <button className={screen.quiet} type="button" onClick={() => setLost(false)}>
            {t("back")}
          </button>
        </div>
      </main>
    );
  }
  if (boxes === null) {
    return (
      <main className={`${screen.screen} ${styles.page}`}>
        <div className={styles.top}>
          <h1>{t("wordsTitle")}</h1>
          <p className={styles.lead}>{t("restoreIntro")}</p>
        </div>
        <div className={styles.middle}>
          <Length
            count={count}
            onPick={(next) => {
              setCount(next);
              clearError();
            }}
          />
        </div>
        <div className={`${screen.actions} ${styles.bottom}`}>
          <button
            className={screen.primary}
            type="button"
            onClick={() => {
              setBoxes(Array<string>(count).fill(""));
              setIndex(0);
            }}
          >
            {t("continue")}
          </button>
          <button className={screen.quiet} type="button" onClick={() => setLost(true)}>
            {t("lostWords")}
          </button>
        </div>
      </main>
    );
  }
  const last = index === boxes.length - 1;
  const accepted = boxes.slice(0, index);
  return (
    <main className={`${screen.screen} ${styles.page}`}>
      <form
        className={styles.stack}
        onSubmit={(event) => {
          event.preventDefault();
          if (!isWord(current)) {
            return;
          }
          if (last) {
            submitPhrase(boxes);
            return;
          }
          setIndex(index + 1);
        }}
      >
        <div className={styles.top}>
          <h1>{t("wordsTitle")}</h1>
          <p className={styles.lead}>{t("wordOf", { n: index + 1, total: boxes.length })}</p>
        </div>
        <div className={styles.middle}>
          <div className={styles.entry}>
            {accepted.length > 0 ? (
              <ol className={styles.done} lang="en" dir="ltr">
                {accepted
                  .map((word, position) => ({ word, position }))
                  .map((line) => (
                    <li key={line.position}>
                      <span className={styles.index}>{line.position + 1}</span>
                      <span>{line.word}</span>
                    </li>
                  ))}
              </ol>
            ) : null}
            <WordField
              id="restore-word"
              label={t("word", { n: index + 1 })}
              value={current}
              active={active}
              onFocus={() => setActive(true)}
              onBlur={() => setActive(false)}
              onChange={(value) => edit(index, value)}
              onPaste={spread}
            />
            {current !== "" && !isPrefix(current) ? (
              <p className={styles.wrong}>{t("wrongWord")}</p>
            ) : null}
          </div>
        </div>
        <Problem />
        <div className={`${screen.actions} ${styles.bottom}`}>
          <button className={screen.primary} type="submit" disabled={!isWord(current) || busy}>
            {last ? t("continue") : t("nextWord")}
          </button>
          <button
            className={screen.quiet}
            type="button"
            onClick={() => {
              if (index === 0) {
                setBoxes(null);
                return;
              }
              setIndex(index - 1);
              setActive(false);
            }}
          >
            {t("back")}
          </button>
        </div>
      </form>
    </main>
  );
}
