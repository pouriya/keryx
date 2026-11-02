import styles from "./session.module.css";
import { useStore } from "./store";
import { useText } from "./text";

export function Problem() {
  const error = useStore((state) => state.error);
  const t = useText();
  if (!error) {
    return null;
  }
  const message =
    error.code === "bad_words"
      ? t("badChecksum")
      : error.code === "wrong_words"
        ? t("wrongWords")
        : error.code === "bad_link"
          ? t("linkUnknown")
          : error.code === "link_used"
            ? t("linkUsed")
            : error.code === "link_expired"
              ? t("linkExpired")
              : error.message;
  return (
    <p className={styles.error} role="alert">
      {message}
    </p>
  );
}
