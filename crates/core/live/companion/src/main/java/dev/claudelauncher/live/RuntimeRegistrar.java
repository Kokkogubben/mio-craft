package dev.claudelauncher.live;

import java.util.HashSet;
import java.util.Map;
import java.util.Set;
import java.util.function.Consumer;

import dev.claudelauncher.live.mixin.HolderReferenceInvoker;
import dev.claudelauncher.live.mixin.MappedRegistryAccessor;
import net.minecraft.core.Registry;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.MinecraftServer;
import net.minecraft.world.item.BlockItem;
import net.minecraft.world.item.Item;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.SoundType;
import net.minecraft.world.level.block.state.BlockBehaviour;
import net.minecraft.world.level.block.state.BlockState;

/**
 * <b>Route 4 — experimental.</b> Registers real blocks and items after the
 * registries are frozen, then patches the lookups Minecraft built from them.
 *
 * <p>Both the client and the integrated server are paused while this runs: the
 * client thread blocks on the server thread, which does the work between ticks.
 *
 * <p>Lookups patched here (each one missed shows up as a glitch or crash):
 * <ol>
 *   <li>registry frozen flag and intrusive holder map (MappedRegistry)</li>
 *   <li>tags on the new holders (unbound tags throw on first tag check)</li>
 *   <li>block state id map and per-state caches (Block.BLOCK_STATE_REGISTRY, initCache)</li>
 *   <li>block → item map (Item.BY_BLOCK)</li>
 *   <li>models and textures: a resource reload, from the claude-live resource pack</li>
 * </ol>
 * Known gaps: other mods' caches built at startup, recipe book, tags (new
 * content is in no tags), and anything multiplayer.
 */
public final class RuntimeRegistrar {
    private RuntimeRegistrar() {}

    /** Set by the client entrypoint; null on dedicated servers (route 4 is singleplayer only). */
    public static volatile Consumer<Runnable> clientExecutor;

    private static final Set<String> DONE = new HashSet<>();

    public static void registerPending(MinecraftServer server) {
        Consumer<Runnable> client = clientExecutor;
        if (client == null || server.isDedicatedServer()) return;
        LiveContent c = LiveContent.current();
        boolean any = c.registeredBlocks().stream().anyMatch(d -> !DONE.contains(d.register_as))
                || c.registeredItems().stream().anyMatch(d -> !DONE.contains(d.register_as));
        if (!any) return;
        client.accept(() -> {
            try {
                // VERIFY(26.x): BlockableEventLoop#executeBlocking
                server.executeBlocking(() -> registerNow(c));
                Textures.reloadPack(net.minecraft.client.Minecraft.getInstance());
            } catch (Throwable t) {
                ClaudeLive.LOG.error("Runtime registration failed", t);
            }
        });
    }

    private static void registerNow(LiveContent c) {
        for (LiveContent.BlockDef d : c.registeredBlocks()) {
            if (DONE.contains(d.register_as)) continue;
            try {
                registerBlock(d);
                DONE.add(d.register_as);
                ClaudeLive.LOG.info("Registered {} at runtime", d.register_as);
            } catch (Throwable t) {
                DONE.add(d.register_as); // don't retry every tick
                ClaudeLive.LOG.error("Couldn't register block {} at runtime", d.register_as, t);
            }
        }
        for (LiveContent.ItemDef d : c.registeredItems()) {
            if (DONE.contains(d.register_as)) continue;
            try {
                registerItem(d);
                DONE.add(d.register_as);
                ClaudeLive.LOG.info("Registered {} at runtime", d.register_as);
            } catch (Throwable t) {
                DONE.add(d.register_as);
                ClaudeLive.LOG.error("Couldn't register item {} at runtime", d.register_as, t);
            }
        }
    }

    @SuppressWarnings({"unchecked", "rawtypes"})
    private static void unfreeze(Registry<?> r) {
        MappedRegistryAccessor a = (MappedRegistryAccessor) (Object) r;
        a.claudeLive$setFrozen(false);
        if (a.claudeLive$getIntrusive() == null) {
            // Blocks and items create an "intrusive" holder in their constructor.
            a.claudeLive$setIntrusive(new java.util.IdentityHashMap());
        }
    }

    private static void refreeze(Registry<?> r) {
        MappedRegistryAccessor a = (MappedRegistryAccessor) (Object) r;
        a.claudeLive$setFrozen(true);
        Map<?, ?> m = a.claudeLive$getIntrusive();
        if (m != null && m.isEmpty()) a.claudeLive$setIntrusive(null);
    }

    @SuppressWarnings("unchecked")
    private static void bindNoTags(Registry<?> r, ResourceKey<?> key) {
        // VERIFY(26.x): Registry#get(ResourceKey) returning Optional<Holder.Reference>
        r.get((ResourceKey) key).ifPresent(h -> ((HolderReferenceInvoker) h).claudeLive$bindTags(java.util.Set.of()));
    }

    private static void registerBlock(LiveContent.BlockDef d) {
        Identifier id = Identifier.parse(d.register_as); // VERIFY(26.x)
        if (BuiltInRegistries.BLOCK.containsKey(id)) return;
        ResourceKey<Block> bk = ResourceKey.create(Registries.BLOCK, id);
        ResourceKey<Item> ik = ResourceKey.create(Registries.ITEM, id);
        unfreeze(BuiltInRegistries.BLOCK);
        unfreeze(BuiltInRegistries.ITEM);
        try {
            LiveBlock block = new LiveBlock(-1, d.register_as, BlockBehaviour.Properties.of()
                    .setId(bk)
                    .strength(Math.max(0, d.hardness), d.resistance)
                    .sound(SoundType.STONE)
                    .lightLevel(s -> s.getValue(LiveBlock.LIGHT)));
            Registry.register(BuiltInRegistries.BLOCK, bk, block);
            LiveBlockItem item = new LiveBlockItem(block, new Item.Properties().setId(ik).useBlockDescriptionPrefix());
            Registry.register(BuiltInRegistries.ITEM, ik, item);

            bindNoTags(BuiltInRegistries.BLOCK, bk);
            bindNoTags(BuiltInRegistries.ITEM, ik);
            for (BlockState s : block.getStateDefinition().getPossibleStates()) {
                Block.BLOCK_STATE_REGISTRY.add(s); // VERIFY(26.x): IdMapper#add
                s.initCache(); // VERIFY(26.x)
            }
            ((BlockItem) item).registerBlocks(Item.BY_BLOCK, item); // VERIFY(26.x)
        } finally {
            refreeze(BuiltInRegistries.ITEM);
            refreeze(BuiltInRegistries.BLOCK);
        }
    }

    private static void registerItem(LiveContent.ItemDef d) {
        Identifier id = Identifier.parse(d.register_as);
        if (BuiltInRegistries.ITEM.containsKey(id)) return;
        ResourceKey<Item> ik = ResourceKey.create(Registries.ITEM, id);
        unfreeze(BuiltInRegistries.ITEM);
        try {
            LiveItem item = new LiveItem(-1, d.register_as, new Item.Properties().setId(ik).stacksTo(Math.max(1, Math.min(99, d.max_stack))));
            Registry.register(BuiltInRegistries.ITEM, ik, item);
            bindNoTags(BuiltInRegistries.ITEM, ik);
        } finally {
            refreeze(BuiltInRegistries.ITEM);
        }
    }
}
