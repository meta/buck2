# Digest Kinds

## Use cases:

- Buck2 needs to support more than just SHA1 for open-sourcing, since publicly
  available RE providers use SHA256.
- Internally, we want to migrate to (potentially keyed) Blake3, and there will
  be a transition period where we need to support both Blake3 and SHA1.

## Proposed plan

Make all the ways in which Buck2 _ingests_ digests either configurable or
explicit about the type of digest they expect.

Internally, we may keep track of digest types for debugging purposes, but we
will never compute more than one digest. It follows that we won't expose
configuration for the digests we _output_ (namely: to use on RE): if we only
have one digest for each blob, making it configurable has no utility since you
never have a choice about the hash to use.

## Implementation

### Hashes received from RE

For interactions with RE, we'll expose two configurations (this can be on the
CommandExecutorConfig):

- Preferred hash to use when Buck2 is doing the hashing (e.g. hashing
  directories).
- Accepted hashes.

We'll use the format of the digests we receive from RE (in particular their
size) to infer what algorithm they used (remember: the RE API provides no way of
knowing the format of a digest, it's just a string).

### Hashes of files

We'll expose the hash to use via a buckconfig. Our
things-that-produce-hashes-of-files should either use the config to choose how
they hash, or fail if they cannot provide the right hash format (e.g. that'll be
true of Eden I/O).

### Hashes of directories

This one gets a little tricky. Our directories currently have an implementation
of fingerprinting that receives only the directory as input, so some refactoring
is in order.

We have two options:

- Pick the hashing algorithm based on the contents of the directory (pick one
  that's already used). Dealing with empty directories is a bit annoying.
- Refactor the directory implementation and have directories parameterized over
  their fingerprints, not their hasher.

The first one is easier but has the downside of not working with keyed Blake3
(because you don't have a way to bring in the key), so I'm aiming for the second
implementation for now.
