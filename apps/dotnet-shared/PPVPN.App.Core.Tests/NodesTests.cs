using PPVPN.App.Core.Backend;
using PPVPN.App.Core.ViewModels;
using PPVPN.Ffi;

namespace PPVPN.App.Core.Tests;

public sealed class NodesTests
{
    [Fact]
    public async Task NodesAreAFlatTableInProfileOrder()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            using var h = new Harness(hooks: MemoryHooks.SignedIn());
            await h.ReadyAsync();
            var nodes = h.Main.Nodes;
            Assert.Equal(NodesViewState.Data, nodes.ViewState);
            Assert.Equal(11, nodes.Items.Count);
            Assert.Equal("nodeCount(n=11)", nodes.NodeCountText);

            var hk1 = nodes.Items[0];
            Assert.Equal(("香港 01", "hk", "专线", true, "中国香港"), (hk1.Name, hk1.CountryCode, hk1.Tier, hk1.HasTier, hk1.Region));
            // Line labels (Replica.label) in failover order.
            Assert.Equal("HKG-A → HKG-B → SZX-R", hk1.Routes);
            Assert.False(nodes.Items[1].HasTier);
            Assert.True(hk1.IsCurrent);
            Assert.False(hk1.SetCurrentCommand.CanExecute(null));
            Assert.Equal(hk1, nodes.SelectedItem);

            // The bottom card follows the selected row.
            nodes.SelectedItem = nodes.Items[3];
            Assert.Equal("nodeProxyT(n=东京 01)", nodes.SelectedNodeProxyTitle);
            Assert.Equal(("u8f2k-jp1", "127.0.0.1:7890"), (nodes.SelectedNodeProxy!.Username, nodes.SelectedNodeProxy.Endpoint));
            nodes.Items[3].CopySocksCommand.Execute(null);
            Assert.Equal($"socks5://u8f2k-jp1:{FakeClientBackend.ProxyPassword}@127.0.0.1:7890", h.Services.Copied.Single());

            // Double-click sets the current node.
            Assert.True(nodes.Items[3].SetCurrentCommand.CanExecute(null));
            await nodes.Items[3].SetCurrentCommand.ExecuteAsync(null);
            await Wait.Until(() => nodes.Items[3].IsCurrent, "current");
            Assert.False(hk1.IsCurrent);
            Assert.Equal("东京 01", h.Main.CurrentNodeName);
            Assert.Equal(nodes.Items[3], h.Main.CurrentNode);
        });
    }

    [Theory]
    [InlineData(0, ProbeMethod.Icmp, "Ping")]
    [InlineData(1, ProbeMethod.Tcp, "TCP")]
    [InlineData(2, ProbeMethod.Connect, "Connect")]
    public async Task ProbeMapsExactlyOneResultToEachNode(int methodIndex, ProbeMethod method, string label)
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            using var h = new Harness(hooks: MemoryHooks.SignedIn());
            await h.ReadyAsync();
            var nodes = h.Main.Nodes;
            nodes.MethodIndex = methodIndex;
            Assert.Equal((method, label), (nodes.Method, nodes.MethodLabel));

            var run = nodes.TestAllCommand.ExecuteAsync(null);
            Assert.True(nodes.IsProbing);
            Assert.Equal($"testingWith(m={label})", nodes.TestingText);
            Assert.All(nodes.Items, i => Assert.Equal(LatencyKind.Testing, i.LatencyKind));
            await run;
            Assert.False(nodes.IsProbing);
            Assert.Equal("", nodes.TestingText);

            foreach (var item in nodes.Items)
            {
                switch (FakeClientBackend.ExpectedFailure(item.Id))
                {
                    case "timeout":
                        Assert.Equal((LatencyKind.Timeout, "timeout", StatusTone.Neutral, "Error_Timeout"), (item.LatencyKind, item.LatencyText, item.LatencyTone, item.LatencyTooltip));
                        break;
                    case "failed":
                        Assert.Equal((LatencyKind.Failed, "failed", StatusTone.Bad), (item.LatencyKind, item.LatencyText, item.LatencyTone));
                        break;
                    default:
                        Assert.Equal(LatencyKind.Value, item.LatencyKind);
                        Assert.Matches(@"^\d+ ms$", item.LatencyText);
                        Assert.Equal(Formatting.LatencyTone(item.LatencyMs!.Value), item.LatencyTone);
                        break;
                }
            }
            Assert.Empty(h.Prompts.Errors);
        });
    }

    [Fact]
    public async Task RowProbeFollowsTheStandardCore()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.SignedInAsync(s => s with { Standard = new StandardState.Starting() });
            var nodes = t.Main.Nodes;
            var item = nodes.Items[0];
            Assert.False(nodes.CanProbe);
            Assert.False(item.CanProbe);
            Assert.False(item.TestOneCommand.CanExecute(null));
            Assert.False(nodes.TestAllCommand.CanExecute(null));
            Assert.Equal("proxyStarting", t.Main.ProxyUnavailableText);
            var changed = new List<string?>();
            var canExecuteChanged = 0;
            item.PropertyChanged += (_, e) => changed.Add(e.PropertyName);
            item.TestOneCommand.CanExecuteChanged += (_, _) => canExecuteChanged++;

            await t.SignedInAsync();
            Assert.True(item.CanProbe);
            Assert.True(item.TestOneCommand.CanExecute(null));
            Assert.True(nodes.TestAllCommand.CanExecute(null));
            Assert.Contains(nameof(NodeItemViewModel.CanProbe), changed);
            Assert.True(canExecuteChanged > 0);

            await t.PushAsync(t.Main.Snapshot with { Standard = new StandardState.Failed(new(ErrorCode.StandardCoreFailed, "exit 1")) });
            Assert.Equal("proxyFailed(reason=Error_StandardCoreFailed)", t.Main.ProxyUnavailableText);
            Assert.Null(t.Main.CurrentNodeProxy);
        });
    }

    [Fact]
    public async Task FailedStandardCoreHidesStaleProxies()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            // The core is ready and the proxies are still loading when it fails.
            t.Backend.ProxiesGate = new(TaskCreationOptions.RunContinuationsAsynchronously);
            await t.PushAsync(ScriptedBackend.SignedIn());
            await Wait.Until(() => t.Main.Nodes.Items.Count > 0, "nodes loaded");
            Assert.False(t.Main.HasCurrentNodeProxy);
            var failed = t.Main.Snapshot with { Standard = new StandardState.Failed(new(ErrorCode.StandardCoreFailed, "apply")) };
            await t.PushAsync(failed);
            t.Backend.ProxiesGate.SetResult();
            await Task.Delay(50);
            var item = t.Main.CurrentNode!;
            Assert.Null(item.Proxy);
            Assert.Null(t.Main.CurrentNodeProxy);
            Assert.Equal("proxyFailed(reason=Error_StandardCoreFailed)", t.Main.ProxyUnavailableText);

            // Even with a proxy still listed on the node, a failed core offers none.
            item.SetProxy(new LocalProxy(item.Id, "127.0.0.1", 7890, "stale", "secret"));
            await t.PushAsync(failed with { Connection = failed.Connection with { Detail = failed.Connection.Detail with { LatencyMs = 40 } } });
            Assert.NotNull(item.Proxy);
            Assert.False(t.Main.HasCurrentNodeProxy);
            Assert.Equal("proxyFailed(reason=Error_StandardCoreFailed)", t.Main.ProxyUnavailableText);

            // Ready again: the proxies come back.
            await t.SignedInAsync();
            Assert.Equal("u8f2k-hk1", t.Main.CurrentNodeProxy!.Username);
            Assert.Null(t.Main.ProxyUnavailableText);
        });
    }

    [Fact]
    public void LatencyThresholds()
    {
        Assert.Equal(StatusTone.Good, Formatting.LatencyTone(99));
        Assert.Equal(StatusTone.Caution, Formatting.LatencyTone(100));
        Assert.Equal(StatusTone.Caution, Formatting.LatencyTone(199));
        Assert.Equal(StatusTone.Bad, Formatting.LatencyTone(200));
    }
}
