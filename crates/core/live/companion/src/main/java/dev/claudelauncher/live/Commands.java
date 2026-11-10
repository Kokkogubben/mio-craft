package dev.claudelauncher.live;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;

import net.minecraft.server.MinecraftServer;

/**
 * Runs commands the launcher queued (one per line in {@code commands.txt}) on
 * the singleplayer server, e.g. {@code give @p claude_live:slot_block_3}.
 * Results are appended to {@code commands.log}.
 */
public final class Commands {
    private Commands() {}

    public static void runQueued(MinecraftServer server) {
        if (server.isDedicatedServer()) return; // singleplayer only
        Path q = ClaudeLive.DIR.resolve("commands.txt");
        Path taken = ClaudeLive.DIR.resolve("commands.running");
        try {
            if (!Files.exists(q)) return;
            Files.move(q, taken, java.nio.file.StandardCopyOption.REPLACE_EXISTING);
            List<String> lines = Files.readAllLines(taken, StandardCharsets.UTF_8);
            Files.deleteIfExists(taken);
            StringBuilder log = new StringBuilder();
            for (String raw : lines) {
                String cmd = raw.strip();
                if (cmd.isEmpty()) continue;
                if (cmd.startsWith("/")) cmd = cmd.substring(1);
                try {
                    // VERIFY(26.x): Commands#performPrefixedCommand and createCommandSourceStack
                    server.getCommands().performPrefixedCommand(server.createCommandSourceStack(), cmd);
                    log.append("ok ").append(cmd).append('\n');
                } catch (Throwable t) {
                    log.append("error ").append(cmd).append(": ").append(t).append('\n');
                }
            }
            Files.writeString(ClaudeLive.DIR.resolve("commands.log"), log.toString(), StandardCharsets.UTF_8,
                    java.nio.file.StandardOpenOption.CREATE, java.nio.file.StandardOpenOption.APPEND);
        } catch (Exception e) {
            ClaudeLive.LOG.warn("Couldn't run queued commands: {}", e.toString());
        }
    }
}
