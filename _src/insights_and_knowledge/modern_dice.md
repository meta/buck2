---
id: modern_dice
title: Introduction to Modern DICE
---

import useBaseUrl from '@docusaurus/useBaseUrl';

# Introduction to Modern Dice

Here is the transcript of our knowledge sharing session on modern dice: <a
href={useBaseUrl("/assets/Modern_DICE.pdf")} target="\_blank">Download
Slides</a>

I will be talking about modern Dice today. I’ll have to get through. I’ll try to
be good with time. So I’ll talk a little bit about what Dice is, show how to use
it, talk a little bit about the internals and what the modern Dice part of that
is.

## What is Dice?

So first, what is Dice? Dice is, you know… We first named this before we started
the buck2 word; we call it Distributed Incremental Computation Engine. So what
does that mean? Computation Engine part. This is, with Dice, you configure this
with Dice by sort of providing us with leaf data and then define a set of
functions that the engine is going to manage for you, right? And then you make a
request. You know, compute function two of A, say, and it will, you know,
that’ll depend on other calls and down to leaf data.

## Dice vs. Standard Programming Functions

You know, at this point, like, is this really any different than just like
Python, right? Python, you define a bunch of functions, you call them, and it
deals with, you know, calling the other functions. At this point, not really,
right? Like, yeah, it does a little bit. So it’ll spawn this work in parallel,
right? It’ll share work if multiple nodes are requesting it. But really, the
interesting parts, I think, when you get to the incremental, the incremental
computation engine.

## Incremental Computation in Dice

So with this, we can invalidate these leaf nodes, right? So invalidate L2 down
there at the bottom. Dice tracks dependencies, and it manages this invalidation
for us, right? So it’s going to invalidate all those reverse dependencies of L2.
Then, say, you compute a new function up here, right? It’s going to, you know,
update the node’s values for everything between, right? So it does, you know, it
does efficient recomputation, right? It only recomputes the nodes that have been
invalidated and need to be recomputed.

## Optimization with Versioning

Has this important optimization that if, as we sort of recompute up one of these
nodes, recomputes the nodes that are invalidated, it only recomputes the nodes
that are invalidated. It only recomputes the nodes that are invalidated. It only
recomputes the nodes that are invalidated. It only recomputes the same value it
had previously, right? We record that. We, you know, the values are recorded
with version numbers. We store the sort of last version that you were valid at.
And then when recomputing a node, even if it was, if it was invalidated, if all
of its dependencies are basically recomputed to the same values, that node will
skip recomputation and just say, oh, it still has the same value as it had at
B2. And so that can, that can, you know, maybe here L2 changed, but X2 and X3
didn’t. And then all the rest would not actually have to do recomputation.

## The “Distributed” Part

The distributed part, there are people who are trying, who have tried to
convince us to change the D. It’s not distributed at this point. You know, it’s
kind of like FSD, right? It’s an aspirational naming. So we’ll get to that.
We’ll get to that maybe next year, next year’s talk.

# Example Walkthrough: Word Count in Recursive Directory

So, yeah, let’s, let’s work through an example. This is, you know, I’m just, you
know, this is sort of a toy example. Let’s, let’s do like word counts of a
recursive directory, right? Say we’ve got, you know, a couple, a couple of
functions for us already, reading files, listing directories, getting word
counts for a string. Pretend they take this long and we’ll, we’ll talk about
sort of the time it takes to, with, with different approaches.

## Naive Approach: Initial Directory Traversal in Rust

Yeah. So this is, this is a pseudo Rust. There’s the, you know, Rust has some
required things that make it a little more proposed. I cut those out. But this
is, this is just sort of a naive approach to this, right? It’s not totally
naive, right? So we’re walking, we’re walking the directory and we’re spawning
off the sort of expensive get word count, right? So this is all done in
parallel. Join the counts at the end and then merge that, right?

## Dice Caching for Improved Efficiency

So, you know, through, through a couple of scenarios together here, right? The
cold one, and then a couple of incremental scenarios in this case, right?
There’s no incrementality. It’s just normal Rust code. So anytime you have to
compute it, it’s going to pay the full cost. I was, you know, I tried to try to
get the numbers here, correct, but I wasn’t super careful. So if, if I’m off a
little bit. So that’s, you know, don’t worry about it, but yeah, so, so the time
here, right? You have, you have something like a, you know, in parallel, you
have like a thousand seconds of, of directory traversal and 10,000 or something
of getting word counts, something like that.

## Implementing Dice Node for Caching Word Count

Okay. So the, the first, the obvious one, if, you know, we have, we have dice,
it does, you know, it’s supposed to do caching for us, let’s cache that
expensive get word count. And so this is, this is what it was. Look like, uh,
with a lot of, with a lot of boilerplate removed of sort of introducing a dice
node to, to cache that word count, right? So we introduced a word count key, and
then we have this key implementation. This key implementation is like the
functions I was referring to in the dice configuration, right? Which tells you
how to take, uh, a key is like the, you know, the input to the function. And
then this compute call is, is the function, this is going to be what dice is
caching and right. All it does is the get word count, uh, which are.

## Updating the Recursive Function for Cached Word Count

Dice read file in here. So, uh, imagine that, uh, read file is one of those
leafs in the, in the image report, right? Uh, and so then using that, we can
update our word count recursive, uh, right. I circled the differences here,
right? We are pushing, we’re collecting a list of files instead of list of, uh,
get word count futures. And then at the end, we do this instead of futures join
all we’re doing this CTX that join all on the dice computations.

## Limitations in Parallelization and Directory Walk

Right. Um, you know, one of the things here, unlike the previous one, the
previous one could start spawning the work early, right? In this case, you see,
actually, we have to do the full directory walk before we spawn the work. Uh,
and that’s due to the dice computations here. If you look at the function
signature, it takes a new reference. Um, if you are working rust, you know, you
can only have sort of one of those at a time. So we wouldn’t be able to spawn
those off early, uh, we don’t want to be able to spawn off one.

## Incremental Caching Benefits and Drawbacks

So, so, okay, great. So we, we, in this case, right. So because of that, uh,
needing to walk first before spawning any work, right. The cold case is actually
slower. Uh, but all, all the add file add or fixed type will rename file. I’ll
get much faster. Right. So if you think, if you think through, okay, so the, the
get word count is going to be cashed. If you add a file, um, the work that’s
going to need to be redone is like, yeah, we have to. First that directory, we
sort of request out all the, all the word counts and we only actually have to
recompute the one new one.

## Optimizing Recursive Spawning and Merging

Uh, and then actually the merge at the end is another, is another thousand
seconds, uh, because it’s emerging, you know, because of these costs that I,
that I threw out there. Uh, so okay. Uh, let’s, let’s see, let’s fix the, let’s
fix the, just the first, like getting it all to spawn in parallel, right? So we
can do that. This is just changing the word count to, um, sort of spawn out
these red spawned out, recursive, right? So, so we spawn out for it, for a
directory, we list it and sub directories, we spawn out the, the recurse for
that and then, and then merge it.

## Enhanced Caching at Directory Level

Right. So this both, uh, does it all in parallel, but also the merge at the end
is not all million items. It’s, you know, just the items for one directory kind
of merge them as we, as we recurse up. Right. Uh, so. I think this is, wait, did
I say that wrong? Uh, we don’t merge them as we recurse up in this case. Do we?
All right. Uh, these, these numbers are a little wrong. I think they should do
mostly a hundred seconds. Um, but again, so this is, this is getting a bit
better.

## Final Optimizations Using Early Cutoff

Uh, but we can, we can do even better than that, right? We can, we can put on
dice, the word counts for each of the directories, right. Um, or recursive for
each directory. And so this looks very much like the, the. Caching it for a
file, right. So it looks the same, uh, I kind of call it a word comp recursive
before here, which is going to be right here, right? And so again, we don’t
change much. We switched to a dice list directory. So we’re caching the, just
the directory listing and then a dice, uh, work count. The recursively cache
dice word count.

## Summary: Efficient Caching with Early Cutoff

Uh, and now our timings get much better right now. Right now, we just, in each
of these cases, I think it ends up being, we are recomputing one, one, uh, sort
of file word count and then just merging them up a set of directories. We can do
a little bit better here, right? So in the fixed typo and rename file case,
right? The actual like word counts of that file or that directory don’t change.
Um, and so that’s, that’s where we should be able to get this early cutoff and
the way. We do that is if you go back to the key implementations, they have this
equality function and you’re right. So we implement the equality function and
then dice while it’s doing the recomputation, we’ll, we’ll apply this early
cutoff optimization for us. Quality here is simple. And so then we get sort of
our best case scenarios for, for each, for each case. Okay. So that, that gets
us through the example.

# Core State Thread and Key Operations

Okay. Uh, great. Looking into the core state thread, really, there are just
three main messages it receives. There’s actually a list of like a dozen or so,
but three main ones are important. The first is an update state message, which
is used for injecting values at the leaves or invalidating them. For each of
these, the core state will invalidate the node, traverse the dependencies, and
increment its version if anything changed.

## Key Lookup and Version Management

The next key operation is look_up_key, which finds a node in the map. If it’s
there and valid, it returns a match; if it’s missing, it returns a compute, or
if it’s invalidated, it triggers dependency checks. Versions increment with each
change, and every computation starts at the newest version, maintaining a
history of node states to track when they were last computed or invalidated.

## Managing Computed State and the Role of Modern Dice

After a compute task finishes, the updated computed column records new values
and dependencies. This structure, where a single-threaded core state manages the
main state, is one of the major changes with Modern Dice. Previously, the main
state was managed within the async evaluator with fine-grained locks, which
became complex to manage, motivating the shift to single-threaded state
management.

## Dependency Checking with Check Depths

Let’s talk about check_depths and how it works. Imagine this as our recursive
word count example. If a file or directory changes, the core state returns
check_depths. In the old Buck approach, all dependencies were checked in
parallel, exiting early if a change was detected, but this could spawn
unnecessary tasks. For instance, if hacking.md was deleted, an invalid key
request could trigger a panic, as seen in Buck’s previous behavior.

## Optimizing Dependency Checks for Performance

The first fix was to check dependencies in order, but this was too slow, as it
led to single-threaded recomputation until changes were found. In Modern Dice,
we use a different approach. With CTX.joinAll, each dependency computation runs
in its own dice computation. Each future created gets its own dependency
tracking, and when joinAll finishes, all dependencies merge into the outer dice
computation as a series-parallel graph, allowing for both ordered and parallel
computations efficiently.

# Series-Parallel Graph Structure for Dependency Tracking

All right. This is kind of a picture of what a series parallel graph looks like.
This isn't related to the recursive word count key stuff. I think maybe it would
be nice if it were, if I had an example of the code here to show it. But, right.
So the idea here would be, you know, this looks like, you know, a computeK1,
computeK2, and then a joinAll splits into three, you know, three inner things.
This could be a join3, right? And so then looking through the top ones, a
computeK3, and then another, say, a join2, et cetera. And then the red dots are
sort of just indicating when the join finishes. Right. So, like, that's the
structure. That's the structure of depths that we are, that we're recording when
we talk about recording depths.

## Benefits of Non-Speculative Dependency Checks

We record them in, like, it's like two flat lists, right? It's recorded as a
flat list of keys and then a flat list of descriptors that describe how to
understand those keys as this graph. And so the checkDepthSpeed3 looks a little
bit more like this. You know, this kind, you know. We iterate through the, we
iterate through sort of the series nodes in the graphs, right? Checking them in
order, exiting out as soon as we get one that's changed. Parallel node, parallel
nodes, we still do the spawn. Maybe I missed a spawn here, but I have the
joinAll. And it turns out, like, with this, right, we won't ever request a key
that the normal compute wouldn't also request. Right. We've sort of tracked the
intra-node data flow or data dependencies, rather. And so one of the great
things that means is that, like, previously, we would have to cancel, right? We
do these checkDepths. It spawns off, you know, it spawns off 10 nodes. Those
spawn off a bunch more. And then we find one change, and we, like, cancel all
that work because, you know, it's all speculative. At this point, it's, like,
not really speculative, right? We know. Even if this. The depth changes, like,
great. That's fine. We do have to redo the compute, and we're going to start
redoing the compute. But doing that compute is going to request all the same
things that we're doing here because the things up to what we're doing here have
been, you know, it will get the same results that it previously had gotten. And
so then we'll do the next things the same. Assuming that your compute is not
non-deterministic, which we've been pretty good about. That's not. We've been
bad about other things, but pretty good about having deterministic piece.

# Internal Workflow of Dice Computations

Let me talk a bit about how this works. So this is, you know, here’s, here’s the
part of the get word count. I cut out the rest, but this is, you do CTX.compute
on this key. Uh, what’s it doing? Uh, it behaves sort of just like you had
called compute, you know, the word count key’s compute code. But internally
there’s a lot going on.

## Overview of Parallel Processing and Task Management

So this is like the rough sketch of this, right? We have a Tokio runtime doing a
whole bunch of things in parallel. We have a whole bunch of different sorts of
computes going on, and each one of them is holding onto one of these dice
computations, which has a depth recording when you call compute. This goes to an
async evaluator, which we have one per transaction—essentially one per larger
computation, like one per buck command.

## Caching Mechanism and State Management

The async evaluator maintains a shared cache of keys to tasks. A task is
essentially about getting the result. The first time we get a compute request,
it spawns the computation and communicates with the core state thread, where the
main cache and state management occur over time. The shared cache is more
temporary, quickly sharing work when multiple requests hit the same key, but the
core state thread manages the main state.

## Processing a Compute Request

Let’s work through how this goes: Dice computations hold the reference to the
compute call, which goes to the async evaluator. The evaluator returns a shared
future; then, we await on it, record the dependency, and return the result to
the user code. For Dice computations, that’s it. It’s straightforward.

## Function of the Async Evaluator

The async evaluator does a get_or_insert on its map, spawning a task for each
requested key once per transaction. The compute task sends a message to the
state thread to look up the key in the state cache. If the state has it cached,
it returns a match to the compute task, which satisfies the future, and anyone
awaiting it receives the result.

## Handling Cache Misses

If the key isn’t in the cache, the core state first returns a compute, then the
compute task calls the key’s compute implementation, retrieves the result, and
sends it back to the core state. The core state may return a different result
than what was sent by the compute task due to reference equality requirements.
It’s essential to return the result instance from the core state, even if
logically equal.

## Handling Invalidated Values

The final case is when the core state had a cached value that became
invalidated. Here, the core state returns check_deps with information about
dependencies that need re-checking. If dependencies have changed, the compute
task recomputes following the same path. If dependencies are unchanged, a
message indicating no changes is sent, providing the same result back.

# Data Flow in Dice Computation

So, yeah. A little bit about how data flows overall into the Dice computation,
right? Like, there’s three ways that it’s intended to flow in, right? We have
injected keys sort of down the leaves at the bottom of the graph as I drew it,
right? This is going to be used for like global data for things that are in like
one state for a particular computation. A really good example of this previously
was buck config, right? The whole buck config would be computed and then put in
a leaf. We’ve actually now, the computation is actually now split and done
partially on Dice, but still the main inputs to buck config are leaves on the
graph.

## Injected Keys and Their Impact

The buck out path, I think, is in an injected key. I think the path to the
prelude, things like this, where it’s like, yeah, we only have one of those.
It’s not really going to, you know, we don’t expect the work. I shouldn’t say
that if the workflow involves like changing one of these a lot, using an
injected key may not be the best thing, right? Because if say something like
buck config, right? If you change buck config, it invalidates basically
everything. And like the workflows for users require changing buck config today.
And so that’s like an unfortunate aspect of the buck config design that sort of
requires that, and like, it’s not, you know, injected key for that. Like, yes,
it’s how you have to do it because that’s the design of buck config, but it’s
not great for the workflow.

## User Data in Dice

The buck out path, right? We don’t, you know, that’s not changing during normal
work. And so like, if the cost of changing that is high, that’s like, you know,
files and directories that act like this, they’re not injected. We don’t inject
the values, but we do like invalidate them. Kind of similar. And like, that is
kind of where data comes in from outside the graph, user data. User data is just
data that we stick on Dice to give us access to things that are useful to have
access to, but aren’t meaningful to the computation. The best example of this is
the event dispatcher, right? Every buck command creates an event dispatcher to
sort of get events back out to the client or to the log. And we, you know, we
put this on the per transaction user data, and keys just access this as much as
they want.

## Keys and Specific Computation Targets

Dice doesn’t really track accesses to user data. And so it’s really only
intended for things that aren’t going to affect the computation. And then the
other way that you might not think about data getting into the computation, but
it really is, is in the data in the keys themselves. For a build, you know, this
might be as little as just what the target is, right. In some sense, Dice could
compute anything, right? It’s like this large infinite graph of possibilities,
and you’re telling it which specific ones you want. I think what people might
think of more as data are BXL files. So BXL files end up in a key for like a BXL
computation. Great. And from those, we form these huge computations of work,
right?

# Best Practices and Key Pitfalls

Great, a little bit about, I don’t know, best practices, things to keep in mind.
I’m not gonna actually, I’m not gonna go through all of these; maybe I’ll leave
this slide up when we get to questions. But the biggest things are like key in
value, key equality especially is where we’ve sort of had the most issues of
getting it incorrect. This is from, you know, one sort of mistake we make is we
will have a tendency, I think, to think about things in terms of like what
states they can be within a command, right? And so within a command, a
configured target node, right? The configured target label is like a unique
identifier for one specific configured target node, right? That’s true within a
command, but when you start comparing keys or values across commands, that’s no
longer true.

## Challenges in Key Equality and State Tracking

If you know, you might make a change, right? You might make a change to
something that’s producing a new configured target node for the same configured
target label. And Dice is gonna want to be able to compare both keys and values
across those states. And so that’s sort of a pitfall we’ve fallen into. Another
is allowing data flow that is not tracked by Dice, right? So if in your key or
in your value, you have a mutex, and you have some mutable state in there, that
can allow data to flow in ways that Dice isn’t tracking, which can cause bad
behavior.

## Avoiding Untracked Data Flows in Keys and Values

An interesting one we had was like a lazily initialized lock, either a once lock
or a lazy if people know the Rust concepts. In one of these, it was in a key,
and we thought that we were doing it correctly, but we basically allowed data
flow that Dice wasn’t tracking correctly. Those are the big things—getting
equality right is critical.

User Data Considerations and Proper Data Flow

And then, like, don’t put things in user data that are essential to the
computation, right? They have to get into the computation in another way, often
just on a leaf in the graph. Often, the things that people want to put in user
data end up being more appropriate just on a leaf in the graph.

## Challenges in Introducing New Data

Introducing new data that flows through keys can be really hard. You can imagine
host info today, available in Starlark, right? You have this host info. You can
imagine wanting to switch that from being an injected key to being in the key
itself and flowing down. And what you find is that you have to flow that
anywhere that a target label goes. Anywhere a target label’s in a key also ends
up needing that info, but it can be hard to introduce new things there.

# Conclusion of Modern Dice Overview

Okay, that’s it. Modern Dice, quick half-hour overview.
