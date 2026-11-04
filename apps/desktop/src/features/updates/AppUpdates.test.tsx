import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { AppUpdates } from "./AppUpdates";

const mocks = vi.hoisted(() => ({ check: vi.fn(), relaunch: vi.fn() }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: mocks.check }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: mocks.relaunch }));

beforeEach(() => { vi.stubEnv("DEV", true); mocks.check.mockReset(); mocks.relaunch.mockReset(); });
afterEach(() => { cleanup(); vi.unstubAllEnvs(); });

async function discover(update: unknown, busy = false) {
  mocks.check.mockResolvedValue(update);
  render(<AppUpdates busy={busy} />);
  fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
  await screen.findByRole("button", { name: "Install update" });
}

describe("signed desktop update flow", () => {
  it("reports when the installed version is current", async () => {
    mocks.check.mockResolvedValue(null);
    render(<AppUpdates busy={false} />);
    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    await screen.findByText("You’re up to date.");
  });

  it("checks automatically in production without installing", async () => {
    vi.stubEnv("DEV", false);
    const install = vi.fn();
    mocks.check.mockResolvedValue({ version: "2.0.0", close: vi.fn(), downloadAndInstall: install });
    render(<AppUpdates busy={false} />);
    await screen.findByText("Orvex 2.0.0 is available.");
    expect(install).not.toHaveBeenCalled();
  });

  it("allows retry after an offline check", async () => {
    mocks.check.mockRejectedValueOnce(new Error("offline")).mockResolvedValue(null);
    render(<AppUpdates busy={false} />);
    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    expect((await screen.findByRole("alert")).textContent).toContain("offline");
    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    await screen.findByText("You’re up to date.");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("shows download bytes and offers restart only after installation", async () => {
    let finish!: () => void;
    const update = { version: "2.0.0", close: vi.fn(), downloadAndInstall: vi.fn((callback) => {
      callback({ event: "Started", data: { contentLength: 2097152 } });
      callback({ event: "Progress", data: { chunkLength: 1048576 } });
      return new Promise<void>((resolve) => { finish = resolve; });
    }) };
    await discover(update);
    fireEvent.click(screen.getByRole("button", { name: "Install update" }));
    await screen.findByText("Downloading 1.0 MB / 2.0 MB");
    expect(screen.getByRole("button", { name: "Updating…" })).toHaveProperty("disabled", true);
    expect(screen.queryByRole("button", { name: "Restart Orvex" })).toBeNull();
    finish();
    fireEvent.click(await screen.findByRole("button", { name: "Restart Orvex" }));
    await waitFor(() => expect(mocks.relaunch).toHaveBeenCalledOnce());
    expect(update.close).toHaveBeenCalledOnce();
  });

  it("keeps a signature failure recoverable and never restarts", async () => {
    const update = { version: "2.0.0", close: vi.fn(), downloadAndInstall: vi.fn().mockRejectedValue(new Error("invalid signature")) };
    await discover(update);
    fireEvent.click(screen.getByRole("button", { name: "Install update" }));
    expect((await screen.findByRole("alert")).textContent).toContain("invalid signature");
    expect(screen.getByRole("button", { name: "Install update" })).toHaveProperty("disabled", false);
    expect(mocks.relaunch).not.toHaveBeenCalled();
  });

  it("blocks installation during a workspace operation", async () => {
    const install = vi.fn();
    await discover({ version: "2.0.0", close: vi.fn(), downloadAndInstall: install }, true);
    expect(screen.getByRole("button", { name: "Install update" })).toHaveProperty("disabled", true);
    fireEvent.click(screen.getByRole("button", { name: "Install update" }));
    expect(install).not.toHaveBeenCalled();
  });
});
