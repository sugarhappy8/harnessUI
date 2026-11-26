// The comparison oracle: stable-diffusion.cpp's own prompt parsing, copied verbatim from the
// engine at the commit Studio pins (sd.lock.json: 532d2c15c430a785a712b2d94281f04e733008a4),
// to check Studio's Rust port (crates/runtime/src/sd/prompt_view.rs) against it. See README.md.
//
// The engine code below (parse_prompt_input and prompt_input_text from
// src/conditioning/prompt_input.cpp, parse_prompt_attention from src/core/util.cpp) is under
// stable-diffusion.cpp's MIT licence:
//
// MIT License
//
// Copyright (c) 2023 leejet
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
//
// extract_and_remove_lora is not the engine's code: it emulates how
// examples/common/common.cpp:2356-2426 removes LoRA tags from the prompt file.

#include <regex>
#include <string>
#include <vector>
#include <cmath>
#include <cctype>
#include <cstdlib>
#include <cstring>
#include <cstdint>
#include <iostream>
#include <iterator>
#include <memory>
#include <stdexcept>
namespace sd {
template <typename T> struct Tensor {};
struct PromptPart {
    enum class Type { TEXT, IMAGE };
    Type type = Type::TEXT;
    std::string content;
    std::shared_ptr<const Tensor<float>> image;
    float weight = 1.f;
};
using PromptInput = std::vector<PromptPart>;
// verbatim: src/conditioning/prompt_input.cpp:69-105 and 113-121
    PromptInput parse_prompt_input(const std::string& text) {
        PromptInput parts;
        std::string plain_text;
        size_t cursor = 0;
        while (cursor < text.size()) {
            const size_t begin = text.find("<image:", cursor);
            if (begin == std::string::npos) {
                plain_text += text.substr(cursor);
                break;
            }
            size_t slashes = 0;
            while (begin > slashes && text[begin - slashes - 1] == '\\') {
                ++slashes;
            }
            if (slashes % 2 != 0) {
                plain_text += text.substr(cursor, begin - cursor - 1);
                plain_text += "<image:";
                cursor = begin + 7;
                continue;
            }
            const size_t end = text.find('>', begin + 7);
            if (end == std::string::npos || end == begin + 7) {
                throw std::invalid_argument("Invalid <image:...> prompt marker at byte " + std::to_string(begin));
            }
            plain_text += text.substr(cursor, begin - cursor);
            if (!plain_text.empty()) {
                parts.push_back({PromptPart::Type::TEXT, std::move(plain_text), {}});
                plain_text.clear();
            }
            parts.push_back({PromptPart::Type::IMAGE, text.substr(begin + 7, end - begin - 7), {}});
            cursor = end + 1;
        }
        if (!plain_text.empty() || parts.empty()) {
            parts.push_back({PromptPart::Type::TEXT, std::move(plain_text), {}});
        }
        return parts;
    }
    std::string prompt_input_text(const PromptInput& input) {
        std::string text;
        for (const auto& part : input) {
            if (part.type == PromptPart::Type::TEXT) {
                text += part.content;
            }
        }
        return text;
    }
}  // namespace sd
// verbatim: src/core/util.cpp:909-1019
std::vector<std::pair<std::string, float>> parse_prompt_attention(const std::string& text) {
    std::vector<std::pair<std::string, float>> res;
    std::vector<int> round_brackets;
    std::vector<int> square_brackets;

    float round_bracket_multiplier  = 1.1f;
    float square_bracket_multiplier = 1 / 1.1f;

    // libstdc++ std::regex recurses per matched character, so unbounded runs
    // overflow the stack. Split runs are merged back below.
    const int max_plain_text_run = 1024;
    std::regex re_attention(R"(\\\(|\\\)|\\\[|\\\]|\\\\|\\|\(|\[|\)|\]|\bBREAK\b|[^\\()\[\]:B]{1,)" +
                            std::to_string(max_plain_text_run) + R"(}|:|\bB)");
    std::regex re_break(R"(\s*\bBREAK\b\s*)");

    auto multiply_range = [&](int start_position, float multiplier) {
        for (int p = start_position; p < res.size(); ++p) {
            res[p].second *= multiplier;
        }
    };

    // Kept out of the regex: bounding the repetition rejects valid long weights,
    // leaving it unbounded overflows the stack.
    auto lex_weight = [](const std::string& s, float& value) -> size_t {
        size_t end = 0;
        if (end < s.size() && (s[end] == '+' || s[end] == '-')) {
            ++end;
        }
        while (end < s.size() && (std::isdigit((unsigned char)s[end]) || s[end] == '.')) {
            ++end;
        }
        if (end >= s.size() || s[end] != ')') {
            return 0;
        }
        std::string number   = s.substr(0, end);
        char* number_end     = nullptr;
        float parsed         = std::strtof(number.c_str(), &number_end);
        const char* expected = number.c_str() + number.size();
        // Without this ".", "+." and "1.2.3" would silently become weights.
        if (number.empty() || number_end != expected || !std::isfinite(parsed)) {
            return 0;
        }
        value = parsed;
        return end + 1;
    };

    std::smatch m, m2;
    std::string remaining_text = text;

    while (std::regex_search(remaining_text, m, re_attention)) {
        std::string text   = m[0];
        std::string suffix = m.suffix();

        if (text == ":") {
            float weight_value   = 1.0f;
            size_t weight_length = lex_weight(suffix, weight_value);
            if (weight_length > 0) {
                if (!round_brackets.empty()) {
                    multiply_range(round_brackets.back(), weight_value);
                    round_brackets.pop_back();
                }
                remaining_text = suffix.substr(weight_length);
                continue;
            }
        }

        if (text == "(") {
            round_brackets.push_back((int)res.size());
        } else if (text == "[") {
            square_brackets.push_back((int)res.size());
        } else if (text == ")" && !round_brackets.empty()) {
            multiply_range(round_brackets.back(), round_bracket_multiplier);
            round_brackets.pop_back();
        } else if (text == "]" && !square_brackets.empty()) {
            multiply_range(square_brackets.back(), square_bracket_multiplier);
            square_brackets.pop_back();
        } else if (text == "\\(") {
            res.push_back({text.substr(1), 1.0f});
        } else if (std::regex_search(text, m2, re_break)) {
            res.push_back({"BREAK", -1.0f});
        } else {
            res.push_back({text, 1.0f});
        }

        remaining_text = suffix;
    }

    for (int pos : round_brackets) {
        multiply_range(pos, round_bracket_multiplier);
    }

    for (int pos : square_brackets) {
        multiply_range(pos, square_bracket_multiplier);
    }

    if (res.empty()) {
        res.push_back({"", 1.0f});
    }

    int i = 0;
    while (i + 1 < res.size()) {
        if (res[i].second == res[i + 1].second) {
            res[i].first += res[i + 1].first;
            res.erase(res.begin() + i + 1);
        } else {
            ++i;
        }
    }

    return res;
}
// the reviewer's emulation of common.cpp:2356-2426's removal
static std::string extract_and_remove_lora(std::string prompt) {
    static const std::regex re(R"(<lora:([^:>]+):([^>]+)>)");
    std::smatch m;
    std::string tmp = prompt;
    while (std::regex_search(tmp, m, re)) {
        prompt = std::regex_replace(prompt, re, "", std::regex_constants::format_first_only);
        tmp = m.suffix().str();
    }
    return prompt;
}
static void put_segs(const std::vector<std::pair<std::string, float>>& segs) {
    for (auto& s : segs) {
        uint32_t bits; std::memcpy(&bits, &s.second, 4);
        std::cout << s.first << '\x01' << bits << '\x02';
    }
}
// input: records "P\x03F\0" (P = text to parse alone, F = prompt file text)
// output per record: parse(P) segments '\x03' then the file's view: THROW | IMAGES | segments, then '\0'
int main() {
    std::string all((std::istreambuf_iterator<char>(std::cin)), {});
    size_t start = 0;
    while (true) {
        size_t end = all.find('\0', start);
        if (end == std::string::npos) break;
        std::string rec = all.substr(start, end - start);
        size_t sep = rec.find('\x03');
        std::string p = rec.substr(0, sep), file = rec.substr(sep + 1);
        put_segs(parse_prompt_attention(p));
        std::cout << '\x03';
        std::string after = extract_and_remove_lora(file);
        try {
            auto input = sd::parse_prompt_input(after);
            bool images = false;
            for (auto& part : input) images |= part.type == sd::PromptPart::Type::IMAGE;
            if (images) std::cout << "IMAGES";
            else put_segs(parse_prompt_attention(sd::prompt_input_text(input)));
        } catch (const std::invalid_argument&) { std::cout << "THROW"; }
        std::cout << '\0';
        start = end + 1;
    }
}
