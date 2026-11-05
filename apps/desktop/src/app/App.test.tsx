import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import App from "./App";

const mocks = vi.hoisted(() => ({ check: vi.fn() }));
vi.mock("../features/updates/checkForUpdates", () => ({ checkForChannelUpdates: mocks.check }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: async (command: string) => {
  if (command === "get_app_info") return { name: "Orvex", version: "0.1.0-beta.3", platform: "macos" };
  if (command === "get_lima_status") return { installed: true };
  if (command === "get_vm_status") return { state: "notCreated" };
  return [];
} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));
vi.mock("../features/workspaces/useMcpStatus", () => ({ useMcpStatus: () => ({ status: null, error: null, refresh: async () => {} }) }));

beforeEach(() => { vi.stubEnv("DEV", true); localStorage.clear(); window.location.hash = ""; mocks.check.mockReset(); });
afterEach(() => { cleanup(); vi.unstubAllEnvs(); });

it("opens settings and preserves an available update across workspace navigation", async () => {
  mocks.check.mockResolvedValue({ version: "0.2.0-beta.1", close: vi.fn(), downloadAndInstall: vi.fn() });
  render(<App />);
  await screen.findByText("Orvex · 0.1.0-beta.3");
  expect(screen.queryByRole("button", { name: "Check for updates" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  expect(screen.getByRole("heading", { name: "Settings" })).toBeTruthy();
  expect(screen.getByText("Version 0.1.0-beta.3 · macOS")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  await screen.findByRole("button", { name: "Install update" });
  fireEvent.click(screen.getByRole("button", { name: /All workspaces/ }));
  expect(screen.queryByRole("button", { name: "Install update" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  expect(screen.getByRole("button", { name: "Install update" })).toBeTruthy();
  expect(mocks.check).toHaveBeenCalledOnce();
});
