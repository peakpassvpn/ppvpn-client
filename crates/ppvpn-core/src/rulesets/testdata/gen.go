//go:build ignore

// Writes the binary rule sets the rulesets tests read, with the sing-box the
// Go core pins (go.mod), as the Go tests build theirs (version 3). From the
// repository root:
//
//	go run ./crates/ppvpn-core/src/rulesets/testdata/gen.go
package main

import (
	"bytes"
	"os"
	"path/filepath"

	"github.com/sagernet/sing-box/common/srs"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/common/json/badoption"
)

func plain(suffixes, cidrs []string) option.HeadlessRule {
	return option.HeadlessRule{Type: C.RuleTypeDefault, DefaultOptions: option.DefaultHeadlessRule{
		DomainSuffix: badoption.Listable[string](suffixes),
		IPCIDR:       badoption.Listable[string](cidrs),
	}}
}

func write(name string, version uint8, rules ...option.HeadlessRule) {
	var buffer bytes.Buffer
	if err := srs.Write(&buffer, option.PlainRuleSet{Rules: rules}, version); err != nil {
		panic(err)
	}
	if err := os.WriteFile(filepath.Join("crates/ppvpn-core/src/rulesets/testdata", name), buffer.Bytes(), 0o644); err != nil {
		panic(err)
	}
}

func main() {
	write("domains.srs", C.RuleSetVersion3, plain([]string{"cn.example"}, nil))
	write("domains-more.srs", C.RuleSetVersion3, plain([]string{"cn.example", "more.example"}, nil))
	write("domains-other.srs", C.RuleSetVersion3, plain([]string{"other.example"}, nil))
	write("cidrs.srs", C.RuleSetVersion3, plain(nil, []string{"1.0.1.0/24"}))
	write("mixed.srs", C.RuleSetVersion3, plain([]string{"cn.example"}, []string{"1.0.1.0/24"}))
	// Every other matcher, then a logical rule holding a CIDR: not mirrored.
	write("logical.srs", C.RuleSetVersion4,
		option.HeadlessRule{Type: C.RuleTypeDefault, DefaultOptions: option.DefaultHeadlessRule{
			QueryType:       badoption.Listable[option.DNSQueryType]{1},
			Network:         badoption.Listable[string]{"tcp"},
			Domain:          badoption.Listable[string]{"exact.example"},
			DomainKeyword:   badoption.Listable[string]{"keyword"},
			DomainRegex:     badoption.Listable[string]{`^regex\.example$`},
			SourceIPCIDR:    badoption.Listable[string]{"192.0.2.0/24"},
			SourcePort:      badoption.Listable[uint16]{1000},
			SourcePortRange: badoption.Listable[string]{"1000:2000"},
			Port:            badoption.Listable[uint16]{443},
			PortRange:       badoption.Listable[string]{"8000:9000"},
			ProcessName:     badoption.Listable[string]{"app"},
			ProcessPath:     badoption.Listable[string]{"/usr/bin/app"},
			PackageName:     badoption.Listable[string]{"com.example.app"},
			WIFISSID:        badoption.Listable[string]{"ssid"},
			WIFIBSSID:       badoption.Listable[string]{"00:00:5e:00:53:00"},
			NetworkType:     badoption.Listable[option.InterfaceType]{option.InterfaceType(C.InterfaceTypeWIFI)},
			Invert:          true,
		}},
		option.HeadlessRule{Type: C.RuleTypeLogical, LogicalOptions: option.LogicalHeadlessRule{
			Mode:  C.LogicalTypeOr,
			Rules: []option.HeadlessRule{plain([]string{"a.example"}, nil), plain(nil, []string{"2001:db8::/32"})},
		}},
	)
	write("adguard.srs", C.RuleSetVersion3, option.HeadlessRule{Type: C.RuleTypeDefault, DefaultOptions: option.DefaultHeadlessRule{
		AdGuardDomain: badoption.Listable[string]{"||ads.example^"},
	}})
}
