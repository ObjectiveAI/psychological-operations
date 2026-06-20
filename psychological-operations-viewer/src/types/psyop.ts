// Type mirrors of psychological-operations-cli/src/psyops/* Rust structs.
// Wire-format-faithful (snake_case fields). Optional fields are `?:` to
// match serde's `skip_serializing_if` predicates — a missing field on
// the wire means "absent" semantically, not "null".
//
// External `Stage` field types (function, profile, strategy) come from
// objectiveai-sdk's `functions/` module; we import the equivalent
// type aliases from `@objectiveai/sdk` so consumers get full type info
// without us re-mirroring objectiveai's own shapes.

import type {
  FunctionsFullInlineFunctionOrRemoteCommitOptional,
  FunctionsInlineProfileOrRemoteCommitOptional,
  FunctionsExecutionsRequestStrategy,
} from "@objectiveai/sdk";

// `psyops list` entry — psychological-operations-cli/src/psyops/mod.rs:121-125
export interface PsyopEntry {
  name: string;
  enabled: boolean;
  commit_sha: string;
}

// `psyops get <name>` payload — psychological-operations-cli/src/psyops/psyop.rs:10-49
export interface Psyop {
  queries?: Query[];
  timeline?: Timeline[];
  mentions?: Mentions[];
  for_you?: ForYou[];
  max_posts: number;
  sort: SortBy;
  fetch_when_for_you_queued: boolean;
  stages?: Stage[];
}

// psychological-operations-cli/src/psyops/query.rs
export interface Query {
  query: string;
  // The agent whose auth this query is scraped as.
  agent_tag: string;
  // Max posts to pull from this query (recent-search paginates up to this).
  max_posts: number;
  priority?: number;
  filter?: Filter;
}

// psychological-operations-cli/src/psyops/timeline.rs
export interface Timeline {
  // The agent whose home timeline this reads, scraped as its auth.
  agent_tag: string;
  // Max posts to pull (paginated).
  max_posts: number;
  priority?: number;
  filter?: Filter;
}

// psychological-operations-cli/src/psyops/mentions.rs
export interface Mentions {
  // The agent whose mentions this reads, scraped as its auth.
  agent_tag: string;
  // Max posts to pull (paginated).
  max_posts: number;
  priority?: number;
  filter?: Filter;
}

// psychological-operations-cli/src/psyops/for_you.rs
export interface ForYou {
  // The agent whose For You feed this entry collects.
  agent_tag: string;
  priority?: number;
  filter?: Filter;
}

// psychological-operations-cli/src/psyops/sort_by.rs:19-32
// Simple variants serialize as plain strings; Custom serializes as an object.
export type SortBy =
  | "likes"
  | "retweets"
  | "replies"
  | "newest"
  | "oldest"
  | { custom: string };

// psychological-operations-cli/src/psyops/filter.rs:23-72
// All fields optional; serde skips Nones.
export interface Filter {
  min_likes?: number;
  max_likes?: number;
  min_likes_per_impression?: number;
  max_likes_per_impression?: number;
  min_retweets?: number;
  max_retweets?: number;
  min_retweets_per_impression?: number;
  max_retweets_per_impression?: number;
  min_replies?: number;
  max_replies?: number;
  min_replies_per_impression?: number;
  max_replies_per_impression?: number;
  min_impressions?: number;
  max_impressions?: number;
  min_age?: number;
  max_age?: number;
  custom?: string;
}

// psychological-operations-cli/src/psyops/stage.rs:17-43
export interface Stage {
  function: FunctionsFullInlineFunctionOrRemoteCommitOptional;
  profile: FunctionsInlineProfileOrRemoteCommitOptional;
  strategy: FunctionsExecutionsRequestStrategy;
  invert: boolean;
  images: boolean;
  videos: boolean;
  output_threshold?: number;
  output_top?: number;
}

// Composite shape `usePsyops` returns: the list entry's metadata
// (name / enabled / commit_sha) flattened together with the full
// on-disk definition under `definition`. The Rust `Psyop` struct
// doesn't carry name/enabled/commit_sha — those live on the entry
// from `psyops list` — so consumers need both.
export interface PsyopWithDefinition {
  name: string;
  enabled: boolean;
  commit_sha: string;
  definition: Psyop;
}
