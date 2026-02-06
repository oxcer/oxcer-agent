export type SessionProfile = "safe" | "balanced" | "experimental";

export type SessionInfo = {
  sessionKey: string;
  title?: string;
  createdAt: string; // ISO timestamp
  updatedAt: string; // ISO timestamp
  favorite?: boolean;
  profile: SessionProfile;
};

export type CreateSessionParams = {
  sessionKey?: string;
  title?: string;
  favorite?: boolean;
  profile?: SessionProfile;
};

export type UpdateSessionPatch = Partial<Pick<SessionInfo, "title" | "favorite" | "profile">>;
