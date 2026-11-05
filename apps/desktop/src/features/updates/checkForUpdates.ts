import { invoke } from "@tauri-apps/api/core";
import { Update } from "@tauri-apps/plugin-updater";

export type UpdateChannel = "stable" | "beta";

export async function checkForChannelUpdates(channel: UpdateChannel): Promise<Update | null> {
  const metadata = await invoke<ConstructorParameters<typeof Update>[0] | null>("check_for_app_update", { channel });
  return metadata ? new Update(metadata) : null;
}
