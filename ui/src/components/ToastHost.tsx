// Toast 宿主（§7.5 v1.167）：右下角堆叠渲染 lib/toast 条目——
// success/info 自动消退（store 侧定时），error 留驻 + ✕ 手关。
// 容器 role=status（error 条目 role=alert）读屏可达；不接 Esc（防误暂停代理）。
import { dismiss, useToasts } from "../lib/toast";
import type { Translate } from "../lib/i18n";

export function ToastHost({ t }: { t: Translate }) {
  const toasts = useToasts();
  if (toasts.length === 0) return null;
  return (
    <div className="toast-host" data-testid="toast-host">
      {toasts.map((item) => (
        <div
          key={item.id}
          className={`toast toast-${item.kind}`}
          role={item.kind === "error" ? "alert" : "status"}
          data-testid={`toast-${item.id}`}
        >
          <span className="toast-msg">{item.message}</span>
          {item.kind === "error" && (
            <button
              type="button"
              className="toast-close"
              aria-label={t("toast.close")}
              data-testid={`toast-close-${item.id}`}
              onClick={() => dismiss(item.id)}
            >
              ✕
            </button>
          )}
        </div>
      ))}
    </div>
  );
}
