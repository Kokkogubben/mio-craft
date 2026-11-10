package dev.miocraft.live;

import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.List;

import net.fabricmc.api.ModInitializer;
import net.fabricmc.fabric.api.event.lifecycle.v1.ServerTickEvents;
import net.fabricmc.fabric.api.itemgroup.v1.FabricItemGroup;
import net.minecraft.core.Registry;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.network.chat.Component;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.MinecraftServer;
import net.minecraft.world.item.CreativeModeTab;
import net.minecraft.world.item.Item;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.SoundType;
import net.minecraft.world.level.block.state.BlockBehaviour;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * MioCraft's live companion mod.
 *
 * <p>Configured by the launcher through system properties:
 * <ul>
 *   <li>{@code miocraft.live.dir} — folder with {@code content.json} and textures</li>
 *   <li>{@code miocraft.live.slots} — number of block and item slots (route 3)</li>
 *   <li>{@code miocraft.live.inMemoryTextures} — paint textures into the atlas</li>
 *   <li>{@code miocraft.live.unfreeze} — allow registering real content at runtime (route 4)</li>
 * </ul>
 *
 * <p>Calls marked {@code VERIFY(26.x)} were written without being able to
 * compile against Minecraft 26.x; check them against the game source.
 */
public final class ClaudeLive implements ModInitializer {
    public static final String MOD_ID = "claude_live";
    public static final Logger LOG = LoggerFactory.getLogger("Claude Live");

    public static final Path DIR = Paths.get(System.getProperty("miocraft.live.dir", "claude-live"));
    public static final int SLOTS = Integer.getInteger("miocraft.live.slots", 0);
    public static final boolean IN_MEMORY_TEXTURES = Boolean.getBoolean("miocraft.live.inMemoryTextures");
    public static final boolean UNFREEZE = Boolean.getBoolean("miocraft.live.unfreeze");

    public static final List<LiveBlock> BLOCKS = new ArrayList<>();
    public static final List<LiveItem> ITEMS = new ArrayList<>();
    public static final List<LiveBlockItem> BLOCK_ITEMS = new ArrayList<>();

    public static Identifier id(String path) {
        return Identifier.fromNamespaceAndPath(MOD_ID, path); // VERIFY(26.x): Identifier factory name
    }

    @Override
    public void onInitialize() {
        try {
            Files.createDirectories(DIR);
        } catch (Exception e) {
            LOG.warn("Couldn't create {}", DIR, e);
        }
        LiveContent.reloadIfChanged();
        registerSlots();
        registerCreativeTab();
        ServerTickEvents.END_SERVER_TICK.register(ClaudeLive::serverTick);
        LOG.info("Claude Live ready: {} slots, in-memory textures {}, registry unfreezing {}",
                SLOTS, IN_MEMORY_TEXTURES, UNFREEZE);
    }

    private static void registerSlots() {
        for (int i = 0; i < SLOTS; i++) {
            Identifier blockId = id("slot_block_" + i);
            ResourceKey<Block> blockKey = ResourceKey.create(Registries.BLOCK, blockId);
            // Each slot gets a live light level property; see LiveBlock.
            LiveBlock block = new LiveBlock(i, BlockBehaviour.Properties.of()
                    .setId(blockKey) // VERIFY(26.x): properties need their key before construction (1.21.2+)
                    .strength(1.5f, 6.0f)
                    .sound(SoundType.STONE)
                    .lightLevel(s -> s.getValue(LiveBlock.LIGHT)));
            Registry.register(BuiltInRegistries.BLOCK, blockKey, block);
            BLOCKS.add(block);

            ResourceKey<Item> blockItemKey = ResourceKey.create(Registries.ITEM, blockId);
            LiveBlockItem bi = new LiveBlockItem(block, new Item.Properties()
                    .setId(blockItemKey) // VERIFY(26.x)
                    .useBlockDescriptionPrefix());
            Registry.register(BuiltInRegistries.ITEM, blockItemKey, bi);
            BLOCK_ITEMS.add(bi);

            Identifier itemId = id("slot_item_" + i);
            ResourceKey<Item> itemKey = ResourceKey.create(Registries.ITEM, itemId);
            LiveItem item = new LiveItem(i, new Item.Properties().setId(itemKey)); // VERIFY(26.x)
            Registry.register(BuiltInRegistries.ITEM, itemKey, item);
            ITEMS.add(item);
        }
    }

    private static void registerCreativeTab() {
        if (SLOTS == 0) return;
        CreativeModeTab tab = FabricItemGroup.builder()
                .title(Component.literal("Claude Live"))
                .icon(() -> new ItemStack(BLOCK_ITEMS.isEmpty() ? ITEMS.get(0) : BLOCK_ITEMS.get(0)))
                .displayItems((params, output) -> {
                    // Only slots that have been given content. Vanilla rebuilds a tab's
                    // contents when it's opened after a change in game state; content
                    // added while the inventory is open shows the next time it opens.
                    LiveContent c = LiveContent.current();
                    for (LiveContent.BlockDef d : c.blocks) {
                        if (d.slot >= 0 && d.slot < BLOCK_ITEMS.size()) output.accept(BLOCK_ITEMS.get(d.slot));
                    }
                    for (LiveContent.ItemDef d : c.items) {
                        if (d.slot >= 0 && d.slot < ITEMS.size()) output.accept(ITEMS.get(d.slot));
                    }
                })
                .build();
        Registry.register(BuiltInRegistries.CREATIVE_MODE_TAB, id("live"), tab);
    }

    private static int tick;

    private static void serverTick(MinecraftServer server) {
        if (++tick % 10 != 0) return;
        boolean changed = LiveContent.reloadIfChanged();
        if (changed && UNFREEZE) {
            RuntimeRegistrar.registerPending(server);
        }
        if (changed) {
            Behaviours.invalidate();
        } else {
            Behaviours.reloadIfClassesChanged();
        }
        Commands.runQueued(server);
    }
}
