/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.cd.workertool.grpc;

import static com.facebook.buck.util.environment.EnvVariablesProvider.getRequiredEnvVar;

import com.facebook.buck.core.util.log.Logger;
import com.facebook.buck.jvm.cd.CompilerDaemonRunner;
import com.facebook.buck.util.Console;
import com.facebook.buck.util.Verbosity;
import com.facebook.buck.util.perf.PerfStatsTracking;
import com.facebook.buck.util.unit.SizeUnit;
import com.facebook.infer.annotation.Nullsafe;
import io.grpc.ForwardingServerCallListener;
import io.grpc.Metadata;
import io.grpc.Server;
import io.grpc.ServerCall;
import io.grpc.ServerCallHandler;
import io.grpc.ServerInterceptor;
import io.grpc.netty.NettyServerBuilder;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.MultithreadEventLoopGroup;
import io.netty.channel.epoll.Epoll;
import io.netty.channel.epoll.EpollEventLoopGroup;
import io.netty.channel.epoll.EpollServerDomainSocketChannel;
import io.netty.channel.kqueue.KQueue;
import io.netty.channel.kqueue.KQueueEventLoopGroup;
import io.netty.channel.kqueue.KQueueServerDomainSocketChannel;
import io.netty.channel.unix.DomainSocketAddress;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.Optional;
import java.util.OptionalLong;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.stream.Collectors;

/** Worker tool grpc server using unix domain sockets */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class WorkerGrpcServer implements ServerInterceptor {
  private static final Logger LOG = Logger.get(WorkerGrpcServer.class);

  // The gRPC server handles sequential compilation requests from a single buck2 client
  // over UDS, so minimal threads suffice. Default Netty thread count is 2x CPU cores
  // (~144 on production hosts), which wastes ~140MB of memory on thread stacks.
  static final int EVENT_LOOP_THREADS = 1;

  String serverName;
  EventLoopGroup eventLoopGroup;
  Server grpcServer;

  /** main function */
  public static void runServer(
      String serverName, WorkerGrpcService.JvmCDCommandFactory commandFactory) {
    Path workerSocketPath = Paths.get(getRequiredEnvVar("WORKER_SOCKET"));

    try {
      Console console = new Console(Verbosity.STANDARD_INFORMATION, System.out, System.err);

      try (CompilerDaemonRunner runner =
          new CompilerDaemonRunner(OutputStream.nullOutputStream(), console)) {
        WorkerGrpcService workerService = new WorkerGrpcService(commandFactory, runner);
        WorkerGrpcServer server = new WorkerGrpcServer(serverName, workerSocketPath, workerService);
        server.run();
      }
    } catch (InterruptedException | IOException e) {
      System.err.println(e.toString());
      System.exit(1);
    }
    System.exit(0);
  }

  public WorkerGrpcServer(
      String serverName, Path unixDomainSocket, WorkerGrpcService workerService) {
    this.serverName = serverName;
    this.grpcServer = buildServer(workerService, unixDomainSocket);
    Runtime.getRuntime()
        .addShutdownHook(
            new Thread(
                () -> {
                  // Use stderr here since the logger may have been reset by its JVM shutdown hook.
                  System.err.println("*** shutting down gRPC server since JVM is shutting down");
                  stopServer(grpcServer);
                  System.err.println("*** server shut down");
                }));

    Executors.newSingleThreadScheduledExecutor(
            r -> {
              Thread t = new Thread(r, serverName + "-health-check");
              t.setDaemon(true);
              return t;
            })
        .scheduleAtFixedRate(this::checkCDState, 1, 10, TimeUnit.SECONDS);
  }

  private static final int AVAILABLE_PROCESSORS = Runtime.getRuntime().availableProcessors();

  // A subreaper such as `systemd --user` adopts an orphaned worker, so its parent need not become
  // pid 1: compare with the parent the worker started with, if it had one.
  private final OptionalLong startingParentPid =
      ProcessHandle.current().parent().stream().mapToLong(ProcessHandle::pid).findFirst();

  private boolean isParentAlive() {
    return isParentAlive(ProcessHandle.current().parent(), startingParentPid);
  }

  static boolean isParentAlive(Optional<ProcessHandle> parent, OptionalLong startingParentPid) {
    return parent.isPresent()
        && (startingParentPid.isEmpty() || parent.get().pid() == startingParentPid.getAsLong())
        && parent.get().pid() != 1
        && parent.get().isAlive();
  }

  private void checkCDState() {
    try {
      logCurrentCDState();
      if (!isParentAlive()) {
        LOG.info("Parent process has been killed, shutting down");
        stopServer(this.grpcServer);
      }
    } catch (Exception e) {
      LOG.error(e);
    }
  }

  private void logCurrentCDState() {
    PerfStatsTracking.MemoryPerfStatsEvent memory = PerfStatsTracking.getMemoryPerfStatsEvent();
    long totalMemoryBytes = memory.getTotalMemoryBytes();
    long freeMemoryBytes = memory.getFreeMemoryBytes();
    long usedMemory = SizeUnit.BYTES.toMegabytes(totalMemoryBytes - freeMemoryBytes);
    long freeMemory = SizeUnit.BYTES.toMegabytes(freeMemoryBytes);
    long totalMemory = SizeUnit.BYTES.toMegabytes(totalMemoryBytes);
    long maxMemory = SizeUnit.BYTES.toMegabytes(memory.getMaxMemoryBytes());
    long timeSpendInGc = TimeUnit.MILLISECONDS.toSeconds(memory.getTimeSpentInGcMs());
    String pools =
        memory.getCurrentMemoryBytesUsageByPool().entrySet().stream()
            .map(e -> e.getKey() + "=" + SizeUnit.BYTES.toMegabytes(e.getValue()))
            .collect(Collectors.joining(", "));

    LOG.info(
        "%s state: Executing tasks: %s, Completed tasks: %s, Available processors: %s, Time spent"
            + " in GC: %s seconds, Used Memory: %s, Free Memory: %s, Total Memory: %s, Max Memory:"
            + " %s, Pools: %s",
        serverName,
        activeTasks.get(),
        completedTasks.get(),
        AVAILABLE_PROCESSORS,
        timeSpendInGc,
        usedMemory,
        freeMemory,
        totalMemory,
        maxMemory,
        pools);
  }

  /** Start the server and wait, clients can connect to the socket after this */
  public void run() throws IOException, InterruptedException {
    LOG.info("Server started");
    grpcServer.start();
    grpcServer.awaitTermination();
  }

  private final AtomicInteger activeTasks = new AtomicInteger(0);
  private final AtomicInteger completedTasks = new AtomicInteger(0);

  @Override
  public <ReqT, RespT> ServerCall.Listener<ReqT> interceptCall(
      ServerCall<ReqT, RespT> call, Metadata headers, ServerCallHandler<ReqT, RespT> next) {
    activeTasks.incrementAndGet();
    return new ForwardingServerCallListener.SimpleForwardingServerCallListener<ReqT>(
        next.startCall(call, headers)) {
      @Override
      public void onComplete() {
        activeTasks.decrementAndGet();
        completedTasks.incrementAndGet();
        super.onComplete();
      }
    };
  }

  private Server buildServer(WorkerGrpcService service, Path unixDomainSocket) {
    LOG.info(
        String.format("Starting %s Worker Server using UDS: %s", serverName, unixDomainSocket));

    DomainSocketAddress socketAddress = new DomainSocketAddress(unixDomainSocket.toString());
    NettyServerBuilder builder =
        NettyServerBuilder.forAddress(socketAddress).addService(service).intercept(this);
    if (KQueue.isAvailable()) {
      this.eventLoopGroup = new KQueueEventLoopGroup(EVENT_LOOP_THREADS);
      builder = builder.channelType(KQueueServerDomainSocketChannel.class);
    } else if (Epoll.isAvailable()) {
      this.eventLoopGroup = new EpollEventLoopGroup(EVENT_LOOP_THREADS);
      builder = builder.channelType(EpollServerDomainSocketChannel.class);
    } else {
      throw new RuntimeException("Server only supports epoll and kqueue (windows not supported)");
    }
    builder = builder.workerEventLoopGroup(eventLoopGroup).bossEventLoopGroup(eventLoopGroup);
    return builder.build();
  }

  int getEventLoopThreadCount() {
    return ((MultithreadEventLoopGroup) eventLoopGroup).executorCount();
  }

  private void stopServer(Server grpcServer) {
    if (grpcServer != null && !grpcServer.isTerminated()) {
      try {
        grpcServer.shutdown().awaitTermination();
        if (!grpcServer.isTerminated()) {
          grpcServer.shutdownNow();
        }
      } catch (InterruptedException e) {
        grpcServer.shutdownNow();
      }
    }
  }
}
