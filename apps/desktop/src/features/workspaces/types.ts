export type AppInfo = { name: string; version: string; platform: string };
export type Workspace = { id: string; name: string; branch: string; baseBranch: string | null; status: string; path: string; repoUrl: string | null; vmName: string | null; agentProviders: string[]; composeFile: string | null };
export type LimaStatus = { installed: boolean; version: string | null; installSupported: boolean; installer: string | null };
export type VmStatus = { state: "notCreated" | "running" | "stopped" | string; instanceName: string; imageBytes: number | null };
export type VmProgress = { phase: string; message: string; downloadedBytes?: number | null; workspaceId?: string };
export type AgentProvider = { id: string; name: string; executable: string; loginHint: string };
export type AgentInstallResult = { providerId: string; providerName: string; installed: boolean; version: string | null; message: string; loginHint: string };
export type AgentPreflight = { workspaceId: string; providerId: string; providerName: string; installed: boolean; version: string | null; message: string; loginHint: string };
export type WorkspaceCreationResult = { workspace: Workspace; agentInstallations: AgentInstallResult[]; setupCommands: string[]; setupError: string | null; setupResult: ProjectSetupResult | null };
export type ProjectSetupResult = { commands: string[]; completed: number; failedCommand: string | null; message: string };
export type WorkspaceServices = {
  workspaceId: string;
  composeFile: string | null;
  dockerInstalled: boolean;
  vmIp: string | null;
  output: string;
  containers: { service: string; state: string; health: string }[];
  ports: { service: string; targetPort: number; publishedPort: number; protocol: string; url: string | null }[];
};
