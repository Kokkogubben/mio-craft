package dev.miocraft.live;

import net.fabricmc.api.ClientModInitializer;
import net.fabricmc.fabric.api.client.event.lifecycle.v1.ClientTickEvents;
import net.minecraft.client.Minecraft;

/** Client side: applies textures and gives the common code a way to run on the client thread. */
public final class ClaudeLiveClient implements ClientModInitializer {
    private static int tick;

    @Override
    public void onInitializeClient() {
        RuntimeRegistrar.clientExecutor = r -> Minecraft.getInstance().execute(r);
        ClientTickEvents.END_CLIENT_TICK.register(mc -> {
            if (++tick % 10 != 0) return;
            LiveContent.reloadIfChanged(); // also needed on the title screen, when no server ticks
            Textures.tick(mc);
        });
    }
}
