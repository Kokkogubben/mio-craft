package dev.claudelauncher.agent;

import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.IOException;
import java.lang.instrument.ClassDefinition;
import java.lang.instrument.ClassFileTransformer;
import java.lang.instrument.Instrumentation;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.nio.file.StandardCopyOption;
import java.security.ProtectionDomain;
import java.security.SecureRandom;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Claude Launcher's hotswap agent. Loaded with
 * {@code -javaagent:claude-live-agent.jar=<path to agent.json>}.
 *
 * <p>It listens on a random localhost port (written to agent.json with a random
 * token) and accepts recompiled classes from the launcher:
 * <ul>
 *   <li>classes already loaded are redefined in place
 *       ({@link Instrumentation#redefineClasses});</li>
 *   <li>classes not loaded yet are kept and handed to the JVM when they load,
 *       so the old bytecode in the mod jar is never used.</li>
 * </ul>
 * With JetBrains Runtime and {@code -XX:+AllowEnhancedClassRedefinition},
 * redefinition may also add or remove methods and fields. Without it only
 * method bodies can change.
 *
 * <p>Protocol (all big-endian, {@link DataOutputStream} encoding):
 * token (UTF), command (UTF). {@code PING} → UTF "{}". {@code REDEFINE}: int n,
 * then n × (UTF binary class name, int length, bytes) → UTF JSON result.
 */
public final class LiveAgent {
    private static Instrumentation inst;
    private static String token;
    /** Internal name (a/b/C) → bytes, for classes that haven't loaded yet. */
    private static final Map<String, byte[]> PENDING = new ConcurrentHashMap<>();

    private LiveAgent() {}

    public static void premain(String args, Instrumentation instrumentation) {
        start(args, instrumentation);
    }

    public static void agentmain(String args, Instrumentation instrumentation) {
        start(args, instrumentation);
    }

    private static void start(String args, Instrumentation instrumentation) {
        inst = instrumentation;
        inst.addTransformer(new PendingTransformer(), false);
        try {
            byte[] raw = new byte[24];
            new SecureRandom().nextBytes(raw);
            StringBuilder sb = new StringBuilder();
            for (byte b : raw) sb.append(String.format("%02x", b));
            token = sb.toString();

            ServerSocket server = new ServerSocket(0, 8, InetAddress.getLoopbackAddress());
            String json = "{\"port\":" + server.getLocalPort()
                    + ",\"token\":\"" + token + "\""
                    + ",\"pid\":" + ProcessHandle.current().pid()
                    + ",\"redefine_supported\":" + inst.isRedefineClassesSupported() + "}";
            if (args != null && !args.isEmpty()) {
                Path out = Paths.get(args);
                if (out.getParent() != null) Files.createDirectories(out.getParent());
                Path tmp = out.resolveSibling(out.getFileName() + ".tmp");
                Files.write(tmp, json.getBytes(StandardCharsets.UTF_8));
                Files.move(tmp, out, StandardCopyOption.REPLACE_EXISTING);
            }
            Thread t = new Thread(() -> serve(server), "claude-live-agent");
            t.setDaemon(true);
            t.start();
            System.out.println("[Claude Launcher] live agent listening on port " + server.getLocalPort());
        } catch (IOException e) {
            System.err.println("[Claude Launcher] live agent failed to start: " + e);
        }
    }

    private static void serve(ServerSocket server) {
        while (true) {
            try (Socket s = server.accept()) {
                s.setSoTimeout(30_000);
                DataInputStream in = new DataInputStream(s.getInputStream());
                DataOutputStream out = new DataOutputStream(s.getOutputStream());
                if (!token.equals(in.readUTF())) {
                    out.writeUTF("{\"error\":\"bad token\"}");
                    continue;
                }
                String cmd = in.readUTF();
                if ("PING".equals(cmd)) {
                    out.writeUTF("{}");
                } else if ("REDEFINE".equals(cmd)) {
                    int n = in.readInt();
                    if (n < 0 || n > 10_000) throw new IOException("bad class count");
                    List<String> names = new ArrayList<>();
                    List<byte[]> bytes = new ArrayList<>();
                    for (int i = 0; i < n; i++) {
                        names.add(in.readUTF());
                        int len = in.readInt();
                        if (len < 0 || len > 64 * 1024 * 1024) throw new IOException("bad class size");
                        byte[] b = new byte[len];
                        in.readFully(b);
                        bytes.add(b);
                    }
                    out.writeUTF(redefine(names, bytes));
                } else {
                    out.writeUTF("{\"error\":\"unknown command\"}");
                }
                out.flush();
            } catch (Throwable e) {
                // Keep serving; a bad request must never take the game down.
                System.err.println("[Claude Launcher] live agent request failed: " + e);
            }
        }
    }

    private static String redefine(List<String> names, List<byte[]> bytes) {
        Map<String, List<Class<?>>> loaded = new java.util.HashMap<>();
        java.util.Set<String> wanted = new java.util.HashSet<>(names);
        for (Class<?> c : inst.getAllLoadedClasses()) {
            if (wanted.contains(c.getName())) loaded.computeIfAbsent(c.getName(), k -> new ArrayList<>()).add(c);
        }
        List<String> redefined = new ArrayList<>();
        List<String> deferred = new ArrayList<>();
        List<String> errors = new ArrayList<>();
        boolean needsRestart = false;
        for (int i = 0; i < names.size(); i++) {
            String name = names.get(i);
            byte[] b = bytes.get(i);
            List<Class<?>> classes = loaded.get(name);
            if (classes == null) {
                PENDING.put(name.replace('.', '/'), b);
                deferred.add(name);
                continue;
            }
            try {
                ClassDefinition[] defs = new ClassDefinition[classes.size()];
                for (int j = 0; j < defs.length; j++) defs[j] = new ClassDefinition(classes.get(j), b);
                inst.redefineClasses(defs);
                redefined.add(name);
            } catch (UnsupportedOperationException e) {
                needsRestart = true;
                errors.add(name + ": this change needs a restart (" + e.getMessage() + ")");
            } catch (Throwable e) {
                errors.add(name + ": " + e);
            }
        }
        return "{\"redefined\":" + list(redefined) + ",\"deferred\":" + list(deferred)
                + ",\"errors\":" + list(errors) + ",\"needs_restart\":" + needsRestart + "}";
    }

    private static String list(List<String> items) {
        StringBuilder sb = new StringBuilder("[");
        for (int i = 0; i < items.size(); i++) {
            if (i > 0) sb.append(',');
            sb.append('"');
            for (char ch : items.get(i).toCharArray()) {
                if (ch == '"' || ch == '\\') sb.append('\\').append(ch);
                else if (ch < 0x20) sb.append(String.format("\\u%04x", (int) ch));
                else sb.append(ch);
            }
            sb.append('"');
        }
        return sb.append(']').toString();
    }

    private static final class PendingTransformer implements ClassFileTransformer {
        @Override
        public byte[] transform(ClassLoader loader, String className, Class<?> redefining,
                                ProtectionDomain domain, byte[] classfile) {
            if (className == null || redefining != null) return null;
            return PENDING.remove(className);
        }
    }
}
