import { type InputHTMLAttributes, useEffect, useState } from "react";
import { Logo } from "./logo";
import { Problem } from "./problem";
import styles from "./session.module.css";
import { submit, useStore } from "./store";
import { useText } from "./text";
import { Words } from "./words";

export function App() {
  const held = useStore((state) => state.held);
  const phase = useStore((state) => state.phase);
  const language = useStore((state) => state.language);
  const t = useText();
  document.documentElement.lang = language;
  document.documentElement.dir = language === "fa" ? "rtl" : "ltr";
  document.title = t("name");
  if (held) {
    return <Elsewhere />;
  }
  if (phase === "words") {
    return <Words />;
  }
  if (phase === "in") {
    return <SignedIn />;
  }
  if (phase === "setup") {
    return <Setup />;
  }
  if (phase === "account") {
    return <Account />;
  }
  return (
    <main className={styles.screen}>
      <Logo />
      <Problem />
    </main>
  );
}

function Field({ label, ...input }: { label: string } & InputHTMLAttributes<HTMLInputElement>) {
  return (
    <div className={styles.field}>
      <input {...input} placeholder=" " />
      <label htmlFor={input.id}>{label}</label>
    </div>
  );
}

function Elsewhere() {
  const t = useText();
  const useHere = useStore((state) => state.useHere);
  return (
    <main className={styles.screen}>
      <Logo />
      <h1>{t("name")}</h1>
      <p className={styles.note}>{t("elsewhere")}</p>
      <div className={styles.actions}>
        <button className={styles.primary} type="button" onClick={useHere}>
          {t("useHere")}
        </button>
      </div>
    </main>
  );
}

function Refresh() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        d="M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5"
      />
    </svg>
  );
}

function useCaptcha() {
  const on = useStore((state) => state.captcha);
  const id = useStore((state) => state.captchaId);
  const image = useStore((state) => state.captchaImage);
  const [answer, setAnswer] = useState("");
  useEffect(() => {
    if (on && image === null) {
      useStore.getState().send({ type: "captcha" });
    }
  }, [on, image]);
  useEffect(() => {
    if (typeof id === "string" || id === null) {
      setAnswer("");
    }
  }, [id]);
  const value =
    on && id !== null && answer.trim() !== "" ? { id, answer: answer.trim() } : undefined;
  return { on, id, image, answer, setAnswer, value };
}

function CaptchaRow({ answer, onAnswer }: { answer: string; onAnswer: (answer: string) => void }) {
  const t = useText();
  const on = useStore((state) => state.captcha);
  const image = useStore((state) => state.captchaImage);
  if (!on) {
    return null;
  }
  return (
    <>
      <div className={styles.captcha}>
        {image !== null ? <img src={image} alt="" /> : <span className={styles.captchaWait} />}
        <button
          className={styles.refresh}
          type="button"
          aria-label={t("newCaptcha")}
          onClick={() => {
            onAnswer("");
            useStore.setState({ captchaId: null, captchaImage: null });
          }}
        >
          <Refresh />
        </button>
      </div>
      <Field
        id="captcha"
        label={t("captcha")}
        name="captcha"
        value={answer}
        autoComplete="off"
        autoCapitalize="none"
        autoCorrect="off"
        spellCheck={false}
        dir="ltr"
        onChange={(event) => onAnswer(event.target.value)}
      />
    </>
  );
}

function Account() {
  const t = useText();
  const busy = useStore((state) => state.busy);
  const registration = useStore((state) => state.registration);
  const registered = useStore((state) => state.registered);
  const picture = useCaptcha();
  const [mode, setMode] = useState<"sign_up" | "log_in">("log_in");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const signingUp = !registered && registration === true && mode === "sign_up";
  return (
    <main className={styles.screen}>
      <Logo />
      <h1>{signingUp ? t("signUp") : t("logIn")}</h1>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          const name = username.trim().toLowerCase();
          if (name === "" || password === "" || (picture.on && picture.value === undefined)) {
            return;
          }
          const captcha = picture.value;
          submit(
            signingUp
              ? { type: "sign_up", username: name, password, captcha }
              : { type: "log_in", username: name, password, captcha },
          );
        }}
      >
        <Field
          id="username"
          label={t("username")}
          name="username"
          value={username}
          autoComplete="username"
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
          dir="ltr"
          onChange={(event) => setUsername(event.target.value)}
        />
        <Field
          id="password"
          label={t("password")}
          name="password"
          type="password"
          value={password}
          autoComplete={signingUp ? "new-password" : "current-password"}
          onChange={(event) => setPassword(event.target.value)}
        />
        <CaptchaRow answer={picture.answer} onAnswer={picture.setAnswer} />
        <Problem />
        <div className={styles.actions}>
          <button className={styles.primary} type="submit" disabled={busy}>
            {signingUp ? t("signUp") : t("logIn")}
          </button>
          {registration === true && !registered ? (
            <button
              className={styles.quiet}
              type="button"
              onClick={() => setMode(signingUp ? "log_in" : "sign_up")}
            >
              {signingUp ? t("logIn") : t("signUp")}
            </button>
          ) : null}
        </div>
      </form>
    </main>
  );
}

function setupToken(): string | null {
  const hash = location.hash.slice(1);
  if (hash === "") {
    return null;
  }
  try {
    const value = decodeURIComponent(hash);
    return value === "" ? null : value;
  } catch {
    return null;
  }
}

function Setup() {
  const t = useText();
  const busy = useStore((state) => state.busy);
  const token = setupToken();
  const picture = useCaptcha();
  const [password, setPassword] = useState("");
  if (token === null) {
    return (
      <main className={styles.screen}>
        <Logo />
        <h1>{t("setupTitle")}</h1>
        <p className={styles.note}>{t("setupMissing")}</p>
      </main>
    );
  }
  return (
    <main className={styles.screen}>
      <Logo />
      <h1>{t("setupTitle")}</h1>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (password === "" || (picture.on && picture.value === undefined)) {
            return;
          }
          submit({ type: "setup", token, password, captcha: picture.value });
        }}
      >
        <Field
          id="setup-password"
          label={t("password")}
          name="password"
          type="password"
          value={password}
          autoComplete="new-password"
          onChange={(event) => setPassword(event.target.value)}
        />
        <CaptchaRow answer={picture.answer} onAnswer={picture.setAnswer} />
        <Problem />
        <div className={styles.actions}>
          <button className={styles.primary} type="submit" disabled={busy}>
            {t("setPassword")}
          </button>
        </div>
      </form>
    </main>
  );
}

function SignedIn() {
  const t = useText();
  const username = useStore((state) => state.username);
  const busy = useStore((state) => state.busy);
  return (
    <main className={styles.screen}>
      <Logo />
      <h1>{t("name")}</h1>
      <p className={styles.note}>{t("signedIn", { name: username })}</p>
      <p className={styles.note}>{t("next")}</p>
      <Problem />
      <div className={styles.actions}>
        <button
          className={styles.quiet}
          type="button"
          disabled={busy}
          onClick={() => submit({ type: "log_out" })}
        >
          {t("logOut")}
        </button>
      </div>
    </main>
  );
}
