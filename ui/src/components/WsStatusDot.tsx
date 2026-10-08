// 连接状态指示（§7.5 v1.167）：顶栏常驻状态点——绿=在线 / 黄脉冲=重连中 /
// 灰=离线（初始未连接）；title / aria-label 随状态；点击强制立即重连
//（打断 1s 退避，App 经重连计数器重建 WS effect）。
import type { Translate } from "../lib/i18n";

export type WsStatus = "offline" | "connecting" | "online";

interface Props {
  status: WsStatus;
  t: Translate;
  onReconnect: () => void;
}

const LABEL_KEYS: Record<WsStatus, string> = {
  online: "topbar.ws_online",
  connecting: "topbar.ws_connecting",
  offline: "topbar.ws_offline",
};

export function WsStatusDot({ status, t, onReconnect }: Props) {
  const label = t(LABEL_KEYS[status]);
  return (
    <button
      type="button"
      className={`ws-dot ws-${status}`}
      data-testid="ws-status"
      data-status={status}
      title={label}
      aria-label={label}
      aria-live="polite"
      onClick={onReconnect}
    />
  );
}
