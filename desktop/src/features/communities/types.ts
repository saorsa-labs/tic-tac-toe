export type Community = {
  /** Opaque x0xd named-group id; also the stable local UI key. */
  id: string;
  groupId: string;
  name: string;
  addedAt: string;
  /**
   * Absolute directory the agent's `~/.buzz/REPOS` symlinks to, so agents
   * work in the user's existing checkouts instead of re-cloning. `~` is
   * expanded to an absolute path before save. Unset = the default real
   * `REPOS` directory inside the nest.
   */
  reposDir?: string;
};
