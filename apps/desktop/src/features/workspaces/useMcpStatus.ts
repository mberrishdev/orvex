import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { McpConnectionStatus } from "./McpStatus";

export function useMcpStatus() {
  const [status, setStatus] = useState<McpConnectionStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const active = useRef(true);
  const busy = useRef(false);
  const refresh = useCallback(async () => {
    if (busy.current) return;
    busy.current = true;
    try {
      const next = await invoke<McpConnectionStatus>("get_mcp_status");
      if (active.current) { setStatus(next); setError(null); }
    } catch (reason) {
      if (active.current) setError(String(reason));
    } finally { busy.current = false; }
  }, []);
  useEffect(() => {
    active.current = true;
    void refresh();
    const timer = window.setInterval(() => void refresh(), 5000);
    return () => { active.current = false; window.clearInterval(timer); };
  }, [refresh]);
  return { status, error, refresh };
}
