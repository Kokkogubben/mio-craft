package dev.miocraft.live;

import java.util.ArrayList;
import java.util.List;

import dev.miocraft.live.api.LiveBlockBehaviour;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.network.chat.Component;
import net.minecraft.network.chat.MutableComponent;
import net.minecraft.resources.Identifier;
import net.minecraft.world.InteractionResult;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.item.Item;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.context.BlockPlaceContext;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockBehaviour;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.StateDefinition;
import net.minecraft.world.level.block.state.properties.IntegerProperty;
import net.minecraft.world.level.storage.loot.LootParams;
import net.minecraft.world.phys.BlockHitResult;

/**
 * A pre-registered block slot whose name, hardness, light, drops and
 * behaviour come from {@link LiveContent} and can change while the game runs.
 *
 * <p>Light level is a block-state property, because Minecraft caches light per
 * state: changing a slot's light applies to blocks placed after the change.
 */
public class LiveBlock extends Block {
    public static final IntegerProperty LIGHT = IntegerProperty.create("light", 0, 15);

    /** Slot number, or -1 for content registered at runtime (route 4). */
    public final int slot;
    /** For runtime-registered content: its id ("namespace:path"). */
    public final String registeredAs;

    public LiveBlock(int slot, BlockBehaviour.Properties props) {
        this(slot, null, props);
    }

    public LiveBlock(int slot, String registeredAs, BlockBehaviour.Properties props) {
        super(props);
        this.slot = slot;
        this.registeredAs = registeredAs;
        registerDefaultState(stateDefinition.any().setValue(LIGHT, 0));
    }

    public LiveContent.BlockDef def() {
        LiveContent c = LiveContent.current();
        if (registeredAs == null) return c.block(slot);
        for (LiveContent.BlockDef d : c.registeredBlocks()) if (registeredAs.equals(d.register_as)) return d;
        return null;
    }

    @Override
    protected void createBlockStateDefinition(StateDefinition.Builder<Block, BlockState> builder) {
        builder.add(LIGHT);
    }

    @Override
    public BlockState getStateForPlacement(BlockPlaceContext ctx) {
        LiveContent.BlockDef d = def();
        int light = d == null ? 0 : Math.max(0, Math.min(15, d.light));
        return defaultBlockState().setValue(LIGHT, light);
    }

    @Override
    public MutableComponent getName() {
        LiveContent.BlockDef d = def();
        return Component.literal(d != null && d.name != null ? d.name
                : registeredAs != null ? registeredAs : "Empty live block " + slot);
    }

    // VERIFY(26.x): signature of BlockBehaviour#getDestroyProgress
    @Override
    protected float getDestroyProgress(BlockState state, Player player, BlockGetter level, BlockPos pos) {
        LiveContent.BlockDef d = def();
        float hardness = d == null ? 1.5f : d.hardness;
        if (hardness < 0) return 0f; // unbreakable
        if (hardness == 0) return 1f;
        boolean harvestable = d == null || !d.requires_tool || player.hasCorrectToolForDrops(state);
        return player.getDestroySpeed(state) / hardness / (harvestable ? 30f : 100f);
    }

    // VERIFY(26.x): signature of BlockBehaviour#getDrops
    @Override
    protected List<ItemStack> getDrops(BlockState state, LootParams.Builder params) {
        LiveContent.BlockDef d = def();
        List<ItemStack> out = new ArrayList<>();
        if (d == null || d.drops == null || d.drops.isEmpty()) {
            out.add(new ItemStack(asItem()));
            return out;
        }
        java.util.Random rng = new java.util.Random();
        for (LiveContent.Drop drop : d.drops) {
            Identifier id = Identifier.tryParse(drop.item); // VERIFY(26.x)
            if (id == null) continue;
            Item item = BuiltInRegistries.ITEM.getValue(id); // VERIFY(26.x): getValue vs get
            int n = drop.min + (drop.max > drop.min ? rng.nextInt(drop.max - drop.min + 1) : 0);
            if (n > 0) out.add(new ItemStack(item, n));
        }
        return out;
    }

    private LiveBlockBehaviour behaviour() {
        LiveContent.BlockDef d = def();
        return d == null ? null : Behaviours.get(d.behaviour, LiveBlockBehaviour.class);
    }

    // VERIFY(26.x): signature of BlockBehaviour#useWithoutItem
    @Override
    protected InteractionResult useWithoutItem(BlockState state, Level level, BlockPos pos, Player player, BlockHitResult hit) {
        LiveBlockBehaviour b = behaviour();
        if (b == null) return InteractionResult.PASS;
        try {
            return b.use(state, level, pos, player, hit);
        } catch (Throwable t) {
            Behaviours.report(t);
            return InteractionResult.PASS;
        }
    }

    @Override
    public void stepOn(Level level, BlockPos pos, BlockState state, Entity entity) {
        LiveBlockBehaviour b = behaviour();
        if (b != null) {
            try {
                b.stepOn(level, pos, state, entity);
            } catch (Throwable t) {
                Behaviours.report(t);
            }
        }
        super.stepOn(level, pos, state, entity);
    }

    @Override
    public void setPlacedBy(Level level, BlockPos pos, BlockState state, LivingEntity placer, ItemStack stack) {
        super.setPlacedBy(level, pos, state, placer, stack);
        LiveBlockBehaviour b = behaviour();
        if (b != null) {
            try {
                b.placed(level, pos, state, placer);
            } catch (Throwable t) {
                Behaviours.report(t);
            }
        }
    }
}
