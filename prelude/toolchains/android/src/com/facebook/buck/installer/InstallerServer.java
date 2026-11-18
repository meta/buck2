/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.installer;

import static com.google.common.util.concurrent.MoreExecutors.directExecutor;

import com.facebook.infer.annotation.Nullsafe;
import com.google.common.util.concurrent.FutureCallback;
import com.google.common.util.concurrent.Futures;
import com.google.common.util.concurrent.SettableFuture;
import io.grpc.Attributes;
import io.grpc.Server;
import io.grpc.ServerTransportFilter;
import io.grpc.netty.shaded.io.grpc.netty.NettyServerBuilder;
import io.grpc.netty.shaded.io.netty.channel.EventLoopGroup;
import io.grpc.netty.shaded.io.netty.channel.nio.NioEventLoopGroup;
import io.grpc.netty.shaded.io.netty.channel.socket.nio.NioServerSocketChannel;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.SocketAddress;
import java.time.Duration;
import java.time.Instant;
import java.util.Optional;
import java.util.concurrent.TimeUnit;
import java.util.logging.Level;
import java.util.logging.Logger;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class InstallerServer extends ServerTransportFilter {
  private static final Logger LOG = Logger.getLogger(InstallerServer.class.getName());

  Server grpcServer;
  boolean clientConnected = false;

  public InstallerServer(InstallCommand installer, int tcpPort, long installTimeoutSeconds) {
    SettableFuture<Void> isDone = SettableFuture.create();
    InstallerService installerService =
        new InstallerService(installer, isDone, installTimeoutSeconds);
    this.grpcServer = buildServer(installerService, tcpPort);
    Futures.addCallback(
        isDone,
        new FutureCallback<Void>() {
          @Override
          public void onSuccess(Void result) {
            LOG.info("Installer Server shutting down...");
            stopServer();
          }

          @Override
          public void onFailure(Throwable t) {
            LOG.log(Level.WARNING, "Execution exception...", t);
          }
        },
        directExecutor());

    Runtime.getRuntime()
        .addShutdownHook(
            new Thread(
                () -> {
                  // Use stderr here since the logger may have been reset by its JVM shutdown hook.
                  System.err.println("*** shutting down gRPC server since JVM is shutting down");
                  stopServer();
                  System.err.println("*** server shut down");
                }));
  }

  public void run() throws IOException, InterruptedException {
    this.grpcServer.start();
    Instant serverStartTime = Instant.now();

    do {
      try {
        if (!isParentAlive()) {
          LOG.info("Parent process has been killed, shutting down");
          stopServer();
        }
        long serverUptime = Duration.between(serverStartTime, Instant.now()).toSeconds();
        if (!this.clientConnected && serverUptime > 20) {
          LOG.info("No client connected for 20 seconds, shutting down");
          stopServer();
        }
      } catch (Exception e) {
        LOG.log(Level.SEVERE, "Error checking parent process liveness", e);
      }
    } while (!this.grpcServer.awaitTermination(10, TimeUnit.SECONDS));
  }

  /** Called when buck2 connects */
  public Attributes transportReady(Attributes transportAttrs) {
    if (this.clientConnected) {
      this.LOG.log(
          Level.SEVERE,
          "Unexpected state, multiple connections detected. Should be 1 connection per server.");
    }
    this.clientConnected = true;
    return transportAttrs;
  }

  /** Called when buck2 disconnects */
  public void transportTerminated(Attributes transportAttrs) {
    this.LOG.log(Level.SEVERE, "Client disconnected, shutting down");
    stopServer();
  }

  /** Build an installer server at requested {@code TCP}. */
  private Server buildServer(InstallerService service, int tcpPort) {
    LOG.info(String.format("Starting Installer Server using TCP on %s", tcpPort));
    return getTCPServer(service, tcpPort);
  }

  private Server getTCPServer(InstallerService service, int tcpPort) {
    EventLoopGroup group = new NioEventLoopGroup();
    SocketAddress addr = new InetSocketAddress("127.0.0.1", tcpPort);

    Server server =
        NettyServerBuilder.forAddress(addr)
            .channelType(NioServerSocketChannel.class)
            .workerEventLoopGroup(group)
            .bossEventLoopGroup(group)
            .addService(service)
            .addTransportFilter(this)
            .build();
    LOG.info(String.format("Starting server listening on TCP port %s", tcpPort));
    return server;
  }

  /** Shuts down installer server. */
  private void stopServer() {
    if (this.grpcServer != null && !this.grpcServer.isTerminated()) {
      try {
        this.grpcServer.shutdown().awaitTermination(5, TimeUnit.SECONDS);
        if (!this.grpcServer.isTerminated()) {
          this.grpcServer.shutdownNow();
        }
      } catch (InterruptedException e) {
        this.grpcServer.shutdownNow();
      }
    }
  }

  private static boolean isParentAlive() {
    Optional<ProcessHandle> parent = ProcessHandle.current().parent();
    return parent.isPresent() && parent.get().pid() != 1 && parent.get().isAlive();
  }
}
